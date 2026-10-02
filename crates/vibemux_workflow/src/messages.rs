//! Supervised communication: typed envelopes, broker admission, delivery
//! states, and inbox ordering (ADR 031 §5).
//!
//! The sender of a message is the participant the broker authenticated from
//! its channel (the attempt whose checkpoint carried the draft, or the
//! operator's control connection), never a role string inside the draft. A
//! worker cannot speak as the supervisor, cannot grant context, and cannot
//! publish verifier receipts. Admission checks audience, policy, contract
//! freshness, source freshness, size, TTL, fan-out, and correlation depth.
//! Delivery is at-least-once with consumer deduplication by message ID and
//! a monotonic per-recipient sequence.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Sha256Digest, SpecIdentifier};

pub const MAX_MESSAGE_BODY_BYTES: usize = 4096;
pub const MAX_MESSAGE_TTL_MS: u64 = 3_600_000;
pub const MAX_CORRELATION_DEPTH: u32 = 4;
pub const MAX_SOURCE_REFS: usize = 16;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Question,
    Answer,
    ContextOffer,
    ContextRequest,
    ContextGrant,
    InterfaceProposal,
    Finding,
    Blocked,
    Progress,
    ArtifactReady,
}

impl MessageKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Answer => "answer",
            Self::ContextOffer => "context_offer",
            Self::ContextRequest => "context_request",
            Self::ContextGrant => "context_grant",
            Self::InterfaceProposal => "interface_proposal",
            Self::Finding => "finding",
            Self::Blocked => "blocked",
            Self::Progress => "progress",
            Self::ArtifactReady => "artifact_ready",
        }
    }

    /// Kinds a worker may originate. Grants come only from the broker or
    /// the supervisor.
    #[must_use]
    pub const fn worker_may_send(self) -> bool {
        !matches!(self, Self::ContextGrant)
    }

    /// Kinds that need the supervisor rather than routine routing.
    #[must_use]
    pub const fn escalates(self) -> bool {
        matches!(self, Self::InterfaceProposal | Self::Blocked)
    }
}

/// An authenticated participant identity.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Participant {
    Operator,
    Supervisor,
    Broker,
    Worker {
        task_key: SpecIdentifier,
        session_id: Uuid,
    },
}

impl Participant {
    #[must_use]
    pub fn task_key(&self) -> Option<&SpecIdentifier> {
        match self {
            Self::Worker { task_key, .. } => Some(task_key),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Operator => "operator".into(),
            Self::Supervisor => "supervisor".into(),
            Self::Broker => "broker".into(),
            Self::Worker { task_key, .. } => format!("worker:{task_key}"),
        }
    }
}

/// A line range of a file in a specific candidate snapshot.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFileRef {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub snapshot_digest: Sha256Digest,
}

/// What a sender asked to send; untrusted until admitted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDraft {
    pub message_id: Uuid,
    pub kind: MessageKind,
    /// Task key of the recipient worker, or `supervisor`.
    pub to: SpecIdentifier,
    /// A sender identity the draft claims, if any; it must match the
    /// authenticated participant.
    pub claimed_sender: Option<String>,
    pub reply_to: Option<Uuid>,
    pub contract_version: u32,
    pub source_refs: Vec<SourceFileRef>,
    pub body: String,
    pub ttl_ms: u64,
}

/// The facts admission needs about one task's current session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskFacts {
    pub task_key: SpecIdentifier,
    pub participant: Participant,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub may_message: Vec<SpecIdentifier>,
    pub max_messages_per_turn: u32,
    /// Digest of the task's latest collected candidate, if any.
    pub current_snapshot: Option<Sha256Digest>,
}

#[derive(Clone, Copy, Debug)]
pub struct AdmissionFacts<'a> {
    pub workflow_id: Uuid,
    pub tasks: &'a [TaskFacts],
    pub now_ms: u64,
    /// Messages this sender already had admitted in its current turn.
    pub sent_this_turn: u32,
    /// Correlation depth of the message this one replies to.
    pub parent_depth: Option<u32>,
    /// Correlation ID of the message this one replies to.
    pub parent_correlation: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageEnvelope {
    pub message_id: Uuid,
    pub workflow_id: Uuid,
    pub sender: Participant,
    pub recipient: Participant,
    pub kind: MessageKind,
    pub correlation_id: Uuid,
    pub reply_to: Option<Uuid>,
    pub depth: u32,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub source_refs: Vec<SourceFileRef>,
    pub body_sha256: Sha256Digest,
    pub body_bytes: u64,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub audience: Vec<Participant>,
    pub escalated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRejection {
    #[error("claimed sender does not match the authenticated channel")]
    ForgedSender,
    #[error("sender is not a participant of this workflow")]
    UnknownSender,
    #[error("recipient is unknown")]
    UnknownRecipient,
    #[error("communication policy does not permit this recipient")]
    NotPermitted,
    #[error("sender may not originate this kind")]
    KindNotAllowedForSender,
    #[error("message refers to a stale contract")]
    StaleContract,
    #[error("message cites a stale source snapshot")]
    StaleSource,
    #[error("message body is empty or too large")]
    InvalidBody,
    #[error("message TTL is out of range")]
    InvalidTtl,
    #[error("per-turn message budget is exhausted")]
    FanOutExceeded,
    #[error("correlation chain is too deep")]
    LoopDepthExceeded,
    #[error("too many source references")]
    TooManySourceRefs,
}

impl MessageRejection {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ForgedSender => "message_forged_sender",
            Self::UnknownSender => "message_unknown_sender",
            Self::UnknownRecipient => "message_unknown_recipient",
            Self::NotPermitted => "message_not_permitted",
            Self::KindNotAllowedForSender => "message_kind_not_allowed",
            Self::StaleContract => "message_stale_contract",
            Self::StaleSource => "message_stale_source",
            Self::InvalidBody => "message_invalid_body",
            Self::InvalidTtl => "message_invalid_ttl",
            Self::FanOutExceeded => "message_fan_out_exceeded",
            Self::LoopDepthExceeded => "message_loop_depth_exceeded",
            Self::TooManySourceRefs => "message_too_many_source_refs",
        }
    }
}

/// Admits a draft from `sender` (authenticated by the caller) or explains
/// the refusal.
pub fn admit(
    draft: &MessageDraft,
    sender: &Participant,
    facts: AdmissionFacts<'_>,
) -> Result<MessageEnvelope, MessageRejection> {
    if let Some(claimed) = &draft.claimed_sender {
        if *claimed != sender.label() {
            return Err(MessageRejection::ForgedSender);
        }
    }
    let sender_task = match sender {
        Participant::Worker { task_key, .. } => {
            let task = facts
                .tasks
                .iter()
                .find(|task| &task.task_key == task_key && &task.participant == sender)
                .ok_or(MessageRejection::UnknownSender)?;
            if !draft.kind.worker_may_send() {
                return Err(MessageRejection::KindNotAllowedForSender);
            }
            Some(task)
        }
        Participant::Operator | Participant::Supervisor | Participant::Broker => None,
    };
    let (recipient, recipient_contract) = if draft.to.as_str() == "supervisor" {
        (Participant::Supervisor, None)
    } else {
        let task = facts
            .tasks
            .iter()
            .find(|task| task.task_key == draft.to)
            .ok_or(MessageRejection::UnknownRecipient)?;
        (task.participant.clone(), Some(task))
    };
    if let Some(task) = sender_task {
        let permitted =
            recipient == Participant::Supervisor || task.may_message.contains(&draft.to);
        if !permitted {
            return Err(MessageRejection::NotPermitted);
        }
        if draft.contract_version != task.contract_version {
            return Err(MessageRejection::StaleContract);
        }
        if facts.sent_this_turn >= task.max_messages_per_turn {
            return Err(MessageRejection::FanOutExceeded);
        }
        for source in &draft.source_refs {
            if task.current_snapshot != Some(source.snapshot_digest) {
                return Err(MessageRejection::StaleSource);
            }
        }
    }
    if draft.body.trim().is_empty()
        || draft.body.len() > MAX_MESSAGE_BODY_BYTES
        || draft.body.contains('\0')
    {
        return Err(MessageRejection::InvalidBody);
    }
    if draft.ttl_ms == 0 || draft.ttl_ms > MAX_MESSAGE_TTL_MS {
        return Err(MessageRejection::InvalidTtl);
    }
    if draft.source_refs.len() > MAX_SOURCE_REFS {
        return Err(MessageRejection::TooManySourceRefs);
    }
    let depth = facts
        .parent_depth
        .map_or(0, |depth| depth.saturating_add(1));
    if depth > MAX_CORRELATION_DEPTH {
        return Err(MessageRejection::LoopDepthExceeded);
    }
    let contract = recipient_contract.or(sender_task);
    let (contract_id, contract_version) = contract.map_or(
        (Sha256Digest::of(b"supervisor"), draft.contract_version),
        |task| (task.contract_id, task.contract_version),
    );
    let mut audience = vec![recipient.clone(), Participant::Supervisor];
    audience.sort();
    audience.dedup();
    Ok(MessageEnvelope {
        message_id: draft.message_id,
        workflow_id: facts.workflow_id,
        sender: sender.clone(),
        recipient,
        kind: draft.kind,
        correlation_id: facts.parent_correlation.unwrap_or(draft.message_id),
        reply_to: draft.reply_to,
        depth,
        contract_id,
        contract_version,
        source_refs: draft.source_refs.clone(),
        body_sha256: Sha256Digest::of(draft.body.as_bytes()),
        body_bytes: draft.body.len() as u64,
        created_at_ms: facts.now_ms,
        expires_at_ms: facts.now_ms.saturating_add(draft.ttl_ms),
        audience,
        escalated: draft.kind.escalates(),
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Admitted,
    Delivered,
    Acknowledged,
    Rejected,
    Expired,
}

impl MessageState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Delivered => "delivered",
            Self::Acknowledged => "acknowledged",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryEvent<'a> {
    Deliver { now_ms: u64 },
    Acknowledge { by: &'a Participant },
    Expire { now_ms: u64 },
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DeliveryError {
    #[error("message expired")]
    Expired,
    #[error("only the recipient may acknowledge")]
    NotRecipient,
    #[error("message was not delivered")]
    NotDelivered,
    #[error("message already reached a terminal state")]
    Terminal,
    #[error("message has not expired yet")]
    NotExpired,
}

impl DeliveryError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Expired => "message_expired",
            Self::NotRecipient => "message_ack_not_recipient",
            Self::NotDelivered => "message_not_delivered",
            Self::Terminal => "message_terminal",
            Self::NotExpired => "message_not_expired",
        }
    }
}

/// The next delivery state. Repeating a transition that already happened
/// (a redelivery or a repeated acknowledgement) returns the same state.
pub fn transition(
    envelope: &MessageEnvelope,
    state: MessageState,
    event: &DeliveryEvent<'_>,
) -> Result<MessageState, DeliveryError> {
    match (state, event) {
        (MessageState::Admitted | MessageState::Delivered, DeliveryEvent::Deliver { now_ms }) => {
            if *now_ms >= envelope.expires_at_ms {
                Err(DeliveryError::Expired)
            } else {
                Ok(MessageState::Delivered)
            }
        }
        (
            MessageState::Delivered | MessageState::Acknowledged,
            DeliveryEvent::Acknowledge { by },
        ) => {
            if **by == envelope.recipient {
                Ok(MessageState::Acknowledged)
            } else {
                Err(DeliveryError::NotRecipient)
            }
        }
        (MessageState::Admitted, DeliveryEvent::Acknowledge { .. }) => {
            Err(DeliveryError::NotDelivered)
        }
        (MessageState::Admitted | MessageState::Delivered, DeliveryEvent::Expire { now_ms }) => {
            if *now_ms >= envelope.expires_at_ms {
                Ok(MessageState::Expired)
            } else {
                Err(DeliveryError::NotExpired)
            }
        }
        (MessageState::Expired, DeliveryEvent::Expire { .. }) => Ok(MessageState::Expired),
        _ => Err(DeliveryError::Terminal),
    }
}

/// A recipient's view of its inbox: messages are consumed in sequence
/// order, duplicates are ignored, and gaps are refused.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InboxCursor {
    delivered_through: u64,
    seen: BTreeSet<Uuid>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InboxDecision {
    Accept,
    Duplicate,
    OutOfOrder { expected: u64 },
}

impl InboxCursor {
    #[must_use]
    pub fn starting_after(delivered_through: u64) -> Self {
        Self {
            delivered_through,
            seen: BTreeSet::new(),
        }
    }

    #[must_use]
    pub const fn delivered_through(&self) -> u64 {
        self.delivered_through
    }

    pub fn offer(&mut self, sequence: u64, message_id: Uuid) -> InboxDecision {
        if self.seen.contains(&message_id) || sequence <= self.delivered_through {
            return InboxDecision::Duplicate;
        }
        let expected = self.delivered_through + 1;
        if sequence != expected {
            return InboxDecision::OutOfOrder { expected };
        }
        self.delivered_through = sequence;
        self.seen.insert(message_id);
        InboxDecision::Accept
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn worker(task: &str, session: u128) -> Participant {
        Participant::Worker {
            task_key: SpecIdentifier::new(task).expect("task key"),
            session_id: Uuid::from_u128(session),
        }
    }

    pub fn tasks() -> Vec<TaskFacts> {
        vec![
            TaskFacts {
                task_key: SpecIdentifier::new("track_a").expect("key"),
                participant: worker("track_a", 1),
                contract_id: Sha256Digest::of(b"contract_a"),
                contract_version: 1,
                may_message: vec![SpecIdentifier::new("track_b").expect("key")],
                max_messages_per_turn: 2,
                current_snapshot: Some(Sha256Digest::of(b"snapshot_a2")),
            },
            TaskFacts {
                task_key: SpecIdentifier::new("track_b").expect("key"),
                participant: worker("track_b", 2),
                contract_id: Sha256Digest::of(b"contract_b"),
                contract_version: 1,
                may_message: vec![SpecIdentifier::new("track_a").expect("key")],
                max_messages_per_turn: 2,
                current_snapshot: None,
            },
        ]
    }

    pub fn draft(to: &str, kind: MessageKind) -> MessageDraft {
        MessageDraft {
            message_id: Uuid::from_u128(77),
            kind,
            to: SpecIdentifier::new(to).expect("to"),
            claimed_sender: None,
            reply_to: None,
            contract_version: 1,
            source_refs: vec![],
            body: "Which error wins for a numeric title: invalid_request or invalid_title?".into(),
            ttl_ms: 60_000,
        }
    }

    pub fn facts(tasks: &[TaskFacts]) -> AdmissionFacts<'_> {
        AdmissionFacts {
            workflow_id: Uuid::from_u128(9),
            tasks,
            now_ms: 1_000,
            sent_this_turn: 0,
            parent_depth: None,
            parent_correlation: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn a_permitted_question_is_admitted_with_authenticated_sender() {
        let tasks = tasks();
        let envelope = admit(
            &draft("track_a", MessageKind::Question),
            &worker("track_b", 2),
            facts(&tasks),
        )
        .expect("admitted");
        assert_eq!(envelope.sender, worker("track_b", 2));
        assert_eq!(envelope.recipient, worker("track_a", 1));
        assert_eq!(envelope.contract_id, Sha256Digest::of(b"contract_a"));
        assert_eq!(envelope.expires_at_ms, 61_000);
        assert!(!envelope.escalated);
        assert!(envelope.audience.contains(&Participant::Supervisor));
    }

    #[test]
    fn forged_senders_and_spoofed_grants_are_refused() {
        let tasks = tasks();
        let mut forged = draft("track_a", MessageKind::Question);
        forged.claimed_sender = Some("supervisor".into());
        assert_eq!(
            admit(&forged, &worker("track_b", 2), facts(&tasks)),
            Err(MessageRejection::ForgedSender)
        );
        // An attempt whose session is not the task's current session.
        assert_eq!(
            admit(
                &draft("track_a", MessageKind::Question),
                &worker("track_b", 99),
                facts(&tasks)
            ),
            Err(MessageRejection::UnknownSender)
        );
        assert_eq!(
            admit(
                &draft("track_a", MessageKind::ContextGrant),
                &worker("track_b", 2),
                facts(&tasks)
            ),
            Err(MessageRejection::KindNotAllowedForSender)
        );
    }

    #[test]
    fn audience_contract_and_source_freshness_are_enforced() {
        let mut tasks = tasks();
        tasks[1].may_message.clear();
        assert_eq!(
            admit(
                &draft("track_a", MessageKind::Question),
                &worker("track_b", 2),
                facts(&tasks)
            ),
            Err(MessageRejection::NotPermitted)
        );
        let tasks = super::fixtures::tasks();
        let mut stale_contract = draft("track_b", MessageKind::Answer);
        stale_contract.contract_version = 0;
        assert_eq!(
            admit(&stale_contract, &worker("track_a", 1), facts(&tasks)),
            Err(MessageRejection::StaleContract)
        );
        let mut stale_source = draft("track_b", MessageKind::Answer);
        stale_source.source_refs = vec![SourceFileRef {
            path: "src/server.mjs".into(),
            start_line: 1,
            end_line: 5,
            snapshot_digest: Sha256Digest::of(b"snapshot_a1"),
        }];
        assert_eq!(
            admit(&stale_source, &worker("track_a", 1), facts(&tasks)),
            Err(MessageRejection::StaleSource)
        );
        stale_source.source_refs[0].snapshot_digest = Sha256Digest::of(b"snapshot_a2");
        admit(&stale_source, &worker("track_a", 1), facts(&tasks)).expect("fresh source");
    }

    #[test]
    fn fan_out_ttl_size_and_loop_depth_are_bounded() {
        let tasks = tasks();
        let mut busy = facts(&tasks);
        busy.sent_this_turn = 2;
        assert_eq!(
            admit(
                &draft("track_a", MessageKind::Question),
                &worker("track_b", 2),
                busy
            ),
            Err(MessageRejection::FanOutExceeded)
        );
        let mut long_ttl = draft("track_a", MessageKind::Question);
        long_ttl.ttl_ms = MAX_MESSAGE_TTL_MS + 1;
        assert_eq!(
            admit(&long_ttl, &worker("track_b", 2), facts(&tasks)),
            Err(MessageRejection::InvalidTtl)
        );
        let mut big = draft("track_a", MessageKind::Question);
        big.body = "x".repeat(MAX_MESSAGE_BODY_BYTES + 1);
        assert_eq!(
            admit(&big, &worker("track_b", 2), facts(&tasks)),
            Err(MessageRejection::InvalidBody)
        );
        let mut deep = facts(&tasks);
        deep.parent_depth = Some(MAX_CORRELATION_DEPTH);
        assert_eq!(
            admit(
                &draft("track_a", MessageKind::Answer),
                &worker("track_b", 2),
                deep
            ),
            Err(MessageRejection::LoopDepthExceeded)
        );
    }

    #[test]
    fn delivery_states_are_idempotent_and_recipient_bound() {
        let tasks = tasks();
        let envelope = admit(
            &draft("track_a", MessageKind::Question),
            &worker("track_b", 2),
            facts(&tasks),
        )
        .expect("admitted");
        let recipient = worker("track_a", 1);
        assert_eq!(
            transition(
                &envelope,
                MessageState::Admitted,
                &DeliveryEvent::Acknowledge { by: &recipient }
            ),
            Err(DeliveryError::NotDelivered)
        );
        let delivered = transition(
            &envelope,
            MessageState::Admitted,
            &DeliveryEvent::Deliver { now_ms: 2_000 },
        )
        .expect("deliver");
        assert_eq!(
            transition(
                &envelope,
                delivered,
                &DeliveryEvent::Deliver { now_ms: 2_001 }
            ),
            Ok(MessageState::Delivered)
        );
        let intruder = worker("track_b", 2);
        assert_eq!(
            transition(
                &envelope,
                delivered,
                &DeliveryEvent::Acknowledge { by: &intruder }
            ),
            Err(DeliveryError::NotRecipient)
        );
        let acknowledged = transition(
            &envelope,
            delivered,
            &DeliveryEvent::Acknowledge { by: &recipient },
        )
        .expect("ack");
        assert_eq!(
            transition(
                &envelope,
                acknowledged,
                &DeliveryEvent::Acknowledge { by: &recipient }
            ),
            Ok(MessageState::Acknowledged)
        );
        assert_eq!(
            transition(
                &envelope,
                MessageState::Admitted,
                &DeliveryEvent::Deliver { now_ms: 61_000 }
            ),
            Err(DeliveryError::Expired)
        );
        assert_eq!(
            transition(
                &envelope,
                MessageState::Admitted,
                &DeliveryEvent::Expire { now_ms: 61_000 }
            ),
            Ok(MessageState::Expired)
        );
        assert_eq!(
            transition(
                &envelope,
                MessageState::Expired,
                &DeliveryEvent::Deliver { now_ms: 61_000 }
            ),
            Err(DeliveryError::Terminal)
        );
    }

    #[test]
    fn inbox_cursor_deduplicates_and_refuses_gaps() {
        let mut cursor = InboxCursor::starting_after(0);
        assert_eq!(
            cursor.offer(2, Uuid::from_u128(2)),
            InboxDecision::OutOfOrder { expected: 1 }
        );
        assert_eq!(cursor.offer(1, Uuid::from_u128(1)), InboxDecision::Accept);
        assert_eq!(
            cursor.offer(1, Uuid::from_u128(1)),
            InboxDecision::Duplicate
        );
        assert_eq!(
            cursor.offer(3, Uuid::from_u128(1)),
            InboxDecision::Duplicate
        );
        assert_eq!(cursor.offer(2, Uuid::from_u128(2)), InboxDecision::Accept);
        assert_eq!(cursor.delivered_through(), 2);
    }
}
