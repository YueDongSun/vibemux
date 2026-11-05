//! Owned callback/receipt futures. Cancellation of select polling never drops
//! accepted work; actor teardown explicitly retires every slot before stdin.
use super::{
    contracts::*,
    input_writer::{InputReceipt, InputWriteOutcome},
};
use std::{
    future::{Future, pending},
    pin::Pin,
};
use tokio::{sync::oneshot, time::Instant};
use uuid::Uuid;
use vibemux_harness::chat_session::ApprovalChallenge;
use vibemux_store::ChatApprovalDeliveryClaim;
use vibemux_types::{
    ChatApprovalId,
    chat_approval::{ChatApproval, ChatApprovalDelivery},
};

type Job = Pin<Box<dyn Future<Output = ApprovalOutcome> + Send>>;

pub(super) struct ActiveApproval {
    pub challenge: ApprovalChallenge,
    pub record: ChatApproval,
    pub deadline: Option<Instant>,
    pub human_waiting: bool,
    pub prepared_frame: Option<vibemux_harness::chat_session::WireFrame>,
}
pub(super) enum ApprovalOutcome {
    InputWritten {
        request_id: Uuid,
        turn_id: Uuid,
        outcome: InputWriteOutcome,
    },
    Admitted {
        challenge: Box<ApprovalChallenge>,
        deadline: Instant,
        result: Result<Box<ChatApproval>, RuntimeError>,
    },
    Decided {
        request: RuntimeApprovalDecision,
        response: oneshot::Sender<Result<ChatApproval, RuntimeError>>,
        result: Result<Box<crate::chat_writer::ChatApprovalDecisionResult>, RuntimeError>,
    },
    Validated {
        request: RuntimeApprovalDecision,
        record: Box<ChatApproval>,
        dispatch: Box<crate::chat_writer::ChatApprovalDispatch>,
        result: Result<(), RuntimeError>,
    },
    PipeWritten {
        challenge_id: Uuid,
        claim: ChatApprovalDeliveryClaim,
        outcome: InputWriteOutcome,
    },
    Delivered {
        challenge_id: Uuid,
        outcome: InputWriteOutcome,
        result: Result<Box<ChatApproval>, RuntimeError>,
    },
    Expired {
        result: Result<Box<ChatApproval>, RuntimeError>,
    },
    Cancelled {
        response: oneshot::Sender<Result<(), RuntimeError>>,
        result: Result<(), RuntimeError>,
    },
}
struct TurnInput {
    request_id: Uuid,
    turn_id: Uuid,
    receipt: InputReceipt,
}

#[derive(Default)]
pub(super) struct ApprovalRuntime {
    pub active: Option<ActiveApproval>,
    operation: Option<Job>,
    cancellation: Option<Job>,
    input: Option<TurnInput>,
}
impl ApprovalRuntime {
    pub fn operation_pending(&self) -> bool {
        self.operation.is_some()
    }
    pub fn pending(&self) -> bool {
        self.operation.is_some() || self.cancellation.is_some() || self.input.is_some()
    }
    pub fn human_waiting(&self) -> bool {
        self.active.as_ref().is_some_and(|p| p.human_waiting)
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.active.as_ref().and_then(|p| p.deadline)
    }
    pub fn abort(&mut self) {
        self.operation = None;
        self.cancellation = None;
        self.input = None;
    }
    pub fn input(
        &mut self,
        request_id: Uuid,
        turn_id: Uuid,
        receipt: InputReceipt,
    ) -> Result<(), RuntimeError> {
        if self.input.is_some() {
            return Err(RuntimeError::NotReady);
        }
        self.input = Some(TurnInput {
            request_id,
            turn_id,
            receipt,
        });
        Ok(())
    }
    pub fn admit(
        &mut self,
        sink: Sink,
        binding: RuntimeBinding,
        challenge: ApprovalChallenge,
        expires_at: time::OffsetDateTime,
        deadline: Instant,
    ) -> Result<(), RuntimeError> {
        if self.active.is_some() || self.operation.is_some() {
            return Err(RuntimeError::NotReady);
        }
        self.operation = Some(Box::pin(async move {
            let result = sink
                .admit_approval(&binding, challenge.clone(), expires_at)
                .await
                .map(Box::new);
            ApprovalOutcome::Admitted {
                challenge: Box::new(challenge),
                deadline,
                result,
            }
        }));
        Ok(())
    }
    pub fn decide(
        &mut self,
        sink: Sink,
        binding: RuntimeBinding,
        request: RuntimeApprovalDecision,
        response: oneshot::Sender<Result<ChatApproval, RuntimeError>>,
    ) -> Result<(), RuntimeError> {
        if self.operation.is_some() {
            let _ = response.send(Err(RuntimeError::NotReady));
            return Ok(());
        }
        let Some(active) = self.active.as_mut() else {
            let _ = response.send(Err(RuntimeError::WrongTurn));
            return Ok(());
        };
        if active.record.approval_id != request.approval_id {
            let _ = response.send(Err(RuntimeError::WrongTurn));
            return Ok(());
        }
        active.human_waiting = false;
        self.operation = Some(Box::pin(async move {
            let result = sink.decide_approval(&binding, request).await.map(Box::new);
            ApprovalOutcome::Decided {
                request,
                response,
                result,
            }
        }));
        Ok(())
    }
    pub fn delivery(
        &mut self,
        claim: ChatApprovalDeliveryClaim,
        challenge_id: Uuid,
        receipt: Option<InputReceipt>,
    ) -> Result<(), RuntimeError> {
        if self.operation.is_some() {
            return Err(RuntimeError::NotReady);
        }
        self.operation = Some(Box::pin(async move {
            let outcome = match receipt {
                Some(receipt) => receipt.await,
                None => InputWriteOutcome::Unknown,
            };
            ApprovalOutcome::PipeWritten {
                challenge_id,
                claim,
                outcome,
            }
        }));
        Ok(())
    }
    pub fn validate(
        &mut self,
        sink: Sink,
        binding: RuntimeBinding,
        request: RuntimeApprovalDecision,
        record: ChatApproval,
        dispatch: crate::chat_writer::ChatApprovalDispatch,
    ) -> Result<(), RuntimeError> {
        if self.operation.is_some() {
            return Err(RuntimeError::NotReady);
        }
        self.operation = Some(Box::pin(async move {
            let result = sink
                .validate_approval_dispatch(&binding, dispatch.evidence())
                .await;
            ApprovalOutcome::Validated {
                request,
                record: Box::new(record),
                dispatch: Box::new(dispatch),
                result,
            }
        }));
        Ok(())
    }
    pub fn complete(
        &mut self,
        sink: Sink,
        binding: RuntimeBinding,
        claim: ChatApprovalDeliveryClaim,
        challenge_id: Uuid,
        outcome: InputWriteOutcome,
    ) -> Result<(), RuntimeError> {
        if self.operation.is_some() {
            return Err(RuntimeError::NotReady);
        }
        self.operation = Some(Box::pin(async move {
            let delivery = if outcome == InputWriteOutcome::Written {
                ChatApprovalDelivery::Written
            } else {
                ChatApprovalDelivery::Unknown
            };
            let result = sink
                .complete_approval(&binding, claim, delivery)
                .await
                .map(Box::new);
            ApprovalOutcome::Delivered {
                challenge_id,
                outcome,
                result,
            }
        }));
        Ok(())
    }
    pub fn expire(&mut self, sink: Sink, binding: RuntimeBinding, id: ChatApprovalId) {
        // A pending claim future may already own a committed capability. Drop
        // its observer, never replay it; same-writer expiry/release reconciles it.
        self.operation = Some(Box::pin(async move {
            ApprovalOutcome::Expired {
                result: sink.expire_approval(&binding, id).await.map(Box::new),
            }
        }));
        if let Some(active) = self.active.as_mut() {
            active.human_waiting = false;
            active.deadline = None;
        }
    }
    pub fn cancel(
        &mut self,
        sink: Sink,
        binding: RuntimeBinding,
        turn: vibemux_harness::chat_session::TurnSummary,
        response: oneshot::Sender<Result<(), RuntimeError>>,
    ) {
        if self.cancellation.is_some() {
            let _ = response.send(Ok(()));
            return;
        }
        if let Some(active) = self.active.as_mut() {
            active.human_waiting = false;
        }
        self.cancellation = Some(Box::pin(async move {
            let result = sink.cancel_turn(&binding, turn).await;
            ApprovalOutcome::Cancelled { response, result }
        }));
    }
    pub async fn next(&mut self) -> ApprovalOutcome {
        tokio::select! {
            outcome=job(&mut self.operation)=>outcome,
            outcome=job(&mut self.cancellation)=>outcome,
            outcome=input(&mut self.input)=>outcome,
        }
    }
}
async fn job(slot: &mut Option<Job>) -> ApprovalOutcome {
    match slot.as_mut() {
        Some(job) => {
            let result = job.await;
            *slot = None;
            result
        }
        None => pending().await,
    }
}
async fn input(slot: &mut Option<TurnInput>) -> ApprovalOutcome {
    let Some(input) = slot.as_mut() else {
        return pending().await;
    };
    let outcome = (&mut input.receipt).await;
    let result = ApprovalOutcome::InputWritten {
        request_id: input.request_id,
        turn_id: input.turn_id,
        outcome,
    };
    *slot = None;
    result
}
