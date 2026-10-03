//! The context broker (ADR 031 §5).
//!
//! A worker's checkpoint messages are untrusted drafts. The broker binds
//! each to the authenticated sender session and its post-turn snapshot,
//! admits or refuses it through the pure rules, persists the envelope
//! through the single writer, and keeps the body in the private outbox.
//! Delivery happens only at a recipient's turn boundary: the message is
//! marked delivered to that exact attempt before the prompt is rendered and
//! acknowledged when the attempt completes. An answer travels with a
//! scoped bundle drawn from the answering task's collected snapshot, never
//! from its live worktree.

use std::collections::BTreeMap;

use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ContentEntry, ContentKind, ContentState, MessageChange, MessageRecord};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    checkpoint::{CheckpointReport, FileRefClaim},
    context_bundle::{
        AttributedClaim, Attribution, BUNDLE_DIGEST_DOMAIN, BundleRequest, ContextBundle,
        FileSelection, SnapshotView, build_bundle, render_bundle_text,
    },
    messages::{
        AdmissionFacts, InboxCursor, InboxDecision, MessageDraft, MessageKind, MessageState,
        Participant, SourceFileRef, TaskFacts, admit,
    },
    receipts::WorkflowReceipt,
    renderer::ContextBlock,
    workflow_record::{ContentStoreMode, WorkflowRecord},
};

use super::{
    candidate::UnitState,
    error::WorkflowError,
    runtime::{RunContext, now_ms},
    state_files::ContentIndexEntry,
    turns::derived_id,
};
use crate::{WriterError, harness_dispatch::call_writer};

/// Default lifetime of a routed message; bounded by the pure maximum.
pub const DEFAULT_MESSAGE_TTL_MS: u64 = 15 * 60 * 1000;
pub const UNKNOWN_REPLY_CODE: &str = "message_unknown_reply";
const MESSAGE_ID_DOMAIN: &str = "vibemux.workflow.message.v1";
const BUNDLE_ID_DOMAIN: &str = "vibemux.workflow.bundle.v1";
const SUPERVISOR_INBOX_DOMAIN: &str = "vibemux.workflow.supervisor_inbox.v1";
const GRANT_BODY_PREFIX: &str = "bundle:";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Delivery refusals that leave the message out of a turn instead of
/// failing the step.
const UNDELIVERABLE_CODES: [&str; 3] = [
    "message_expired",
    "message_terminal",
    "store_workflow_message_redelivery",
];

/// One admitted message and what travels with it to a turn.
#[derive(Clone, Debug)]
pub(crate) struct Delivery {
    pub record: MessageRecord,
    /// Rendered as turn data, never as instructions.
    pub note: Option<String>,
    pub block: Option<ContextBlock>,
    /// Outbox entries removed once the recipient acknowledged.
    pub outbox: Vec<Uuid>,
}

impl Delivery {
    #[must_use]
    pub fn message_id(&self) -> Uuid {
        self.record.envelope.message_id
    }
}

/// A message the broker admitted from one step.
#[derive(Clone, Debug)]
pub(crate) struct AdmittedMessage {
    pub record: MessageRecord,
    pub body: String,
    /// Admitted by this call rather than found from an earlier run of the
    /// same step.
    pub fresh: bool,
}

impl AdmittedMessage {
    #[must_use]
    pub fn kind(&self) -> MessageKind {
        self.record.envelope.kind
    }

    #[must_use]
    pub fn recipient_task(&self) -> Option<&SpecIdentifier> {
        self.record.envelope.recipient.task_key()
    }
}

/// Marks a message delivered to `attempt`. `false` when it can no longer
/// be delivered to this attempt.
pub(crate) async fn deliver(
    context: &RunContext,
    message_id: Uuid,
    attempt: Uuid,
) -> Result<bool, WorkflowError> {
    let writer = context.writer.clone();
    match call_writer(move || {
        writer.change_workflow_message(
            message_id,
            MessageChange::Deliver { attempt },
            OffsetDateTime::now_utc(),
        )
    })
    .await
    {
        Ok(_) => Ok(true),
        Err(WriterError::Store { code }) if UNDELIVERABLE_CODES.contains(&code.as_str()) => {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

/// Acknowledges delivered messages as their recipient and removes their
/// private outbox entries.
pub(crate) async fn acknowledge(
    context: &RunContext,
    recipient: &Participant,
    delivered: &[Delivery],
    attempt: Uuid,
) -> Result<(), WorkflowError> {
    for delivery in delivered {
        let writer = context.writer.clone();
        let by = recipient.clone();
        let message_id = delivery.message_id();
        call_writer(move || {
            writer.change_workflow_message(
                message_id,
                MessageChange::Acknowledge {
                    by: by.clone(),
                    attempt,
                },
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        let state = context.setup.state.clone();
        let workflow_id = context.workflow_id;
        let outbox = delivery.outbox.clone();
        tokio::task::spawn_blocking(move || {
            outbox
                .iter()
                .try_for_each(|id| state.delete_outbox(workflow_id, *id))
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;
    }
    Ok(())
}

/// Admission facts of every unit's current session.
fn task_facts(
    context: &RunContext,
    record: &WorkflowRecord,
    units: &[&UnitState],
) -> Vec<TaskFacts> {
    units
        .iter()
        .filter_map(|state| {
            let task = record.task(&state.unit.task_key)?;
            let spec = context.plan.specs.get(&state.unit.task_key)?;
            Some(TaskFacts {
                task_key: state.unit.task_key.clone(),
                participant: state.unit.participant(),
                contract_id: task.contract_id,
                contract_version: task.contract_version,
                may_message: spec.communication_policy.may_message.clone(),
                max_messages_per_turn: spec.communication_policy.max_messages_per_turn,
                current_snapshot: state.latest.as_ref().map(|latest| latest.digest()),
            })
        })
        .collect()
}

/// Admits the checkpoint messages of one step of `sender`. `answering`
/// lists the messages delivered in that step: an answer without
/// `reply_to` replies to the question its recipient asked.
pub(crate) async fn admit_step_messages(
    context: &RunContext,
    units: &[&UnitState],
    sender: &UnitState,
    attempt_request_id: Uuid,
    checkpoint: &CheckpointReport,
    answering: &[MessageRecord],
) -> Result<Vec<AdmittedMessage>, WorkflowError> {
    let record = context.record().await?;
    let facts = task_facts(context, &record, units);
    let participant = sender.unit.participant();
    let contract_version = record
        .task(&sender.unit.task_key)
        .ok_or(WorkflowError::Internal)?
        .contract_version;
    let snapshot = sender.latest.as_ref().map(|latest| latest.digest());
    let mut admitted = Vec::new();
    for (index, outgoing) in checkpoint.messages.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| WorkflowError::Internal)?;
        let message_id = derived_id(
            MESSAGE_ID_DOMAIN,
            &[attempt_request_id.as_bytes(), &index.to_be_bytes()],
        );
        let writer = context.writer.clone();
        if let Some(existing) = call_writer(move || writer.workflow_message(message_id)).await? {
            let body = read_body(context, &existing)?;
            admitted.push(AdmittedMessage {
                record: existing,
                body,
                fresh: false,
            });
            continue;
        }
        let reply_to = outgoing.reply_to.or_else(|| {
            (outgoing.kind == MessageKind::Answer)
                .then(|| {
                    answering
                        .iter()
                        .find(|question| {
                            question.envelope.kind == MessageKind::Question
                                && question.envelope.sender.task_key() == Some(&outgoing.to)
                        })
                        .map(|question| question.envelope.message_id)
                })
                .flatten()
        });
        let parent = match reply_to {
            Some(parent_id) => {
                let writer = context.writer.clone();
                match call_writer(move || writer.workflow_message(parent_id)).await? {
                    Some(parent) if parent.envelope.workflow_id == context.workflow_id => {
                        Some(parent)
                    }
                    _ => {
                        reject(context, message_id, &participant, UNKNOWN_REPLY_CODE).await?;
                        continue;
                    }
                }
            }
            None => None,
        };
        let draft = MessageDraft {
            message_id,
            kind: outgoing.kind,
            to: outgoing.to.clone(),
            claimed_sender: outgoing.claimed_sender.clone(),
            reply_to,
            contract_version,
            source_refs: source_refs(&outgoing.source_refs, snapshot),
            body: outgoing.text.clone(),
            ttl_ms: DEFAULT_MESSAGE_TTL_MS,
        };
        let sent_this_turn = u32::try_from(admitted.len()).map_err(|_| WorkflowError::Internal)?;
        let envelope = match admit(
            &draft,
            &participant,
            AdmissionFacts {
                workflow_id: context.workflow_id,
                tasks: &facts,
                now_ms: now_ms(),
                sent_this_turn,
                parent_depth: parent.as_ref().map(|parent| parent.envelope.depth),
                parent_correlation: parent.as_ref().map(|parent| parent.envelope.correlation_id),
            },
        ) {
            Ok(envelope) => envelope,
            Err(rejection) => {
                reject(context, message_id, &participant, rejection.code()).await?;
                continue;
            }
        };
        let state = context.setup.state.clone();
        let workflow_id = context.workflow_id;
        let body = draft.body.clone();
        tokio::task::spawn_blocking(move || state.put_outbox(workflow_id, message_id, &body))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let writer = context.writer.clone();
        let commit = call_writer(move || {
            writer.admit_workflow_message(envelope.clone(), OffsetDateTime::now_utc())
        })
        .await?;
        admitted.push(AdmittedMessage {
            fresh: commit.event.is_some(),
            record: commit.message,
            body: draft.body,
        });
    }
    Ok(admitted)
}

fn source_refs(claims: &[FileRefClaim], snapshot: Option<Sha256Digest>) -> Vec<SourceFileRef> {
    // A claim without a collected snapshot cannot be bound; it is bound to
    // a digest no task holds, so admission refuses it as stale.
    let digest = snapshot.unwrap_or_else(|| Sha256Digest::of(b"vibemux.workflow.no_snapshot"));
    claims
        .iter()
        .map(|claim| SourceFileRef {
            path: claim.path.clone(),
            start_line: claim.start_line,
            end_line: claim.end_line,
            snapshot_digest: digest,
        })
        .collect()
}

async fn reject(
    context: &RunContext,
    message_id: Uuid,
    sender: &Participant,
    code: &str,
) -> Result<(), WorkflowError> {
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    let sender = sender.clone();
    let code = code.to_string();
    call_writer(move || {
        writer.record_workflow_message_rejection(
            workflow_id,
            message_id,
            sender.clone(),
            code.clone(),
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(())
}

fn read_body(context: &RunContext, message: &MessageRecord) -> Result<String, WorkflowError> {
    context
        .setup
        .state
        .read_outbox(
            context.workflow_id,
            message.envelope.message_id,
            message.envelope.body_sha256,
        )?
        .ok_or(WorkflowError::ShareInvalid)
}

/// Routes supervisor-bound messages: the supervisor inbox is the
/// coordinator itself, so they are delivered and acknowledged at once.
pub(crate) async fn settle_supervisor_messages(
    context: &RunContext,
    admitted: &[AdmittedMessage],
) -> Result<(), WorkflowError> {
    for message in admitted
        .iter()
        .filter(|message| message.record.envelope.recipient == Participant::Supervisor)
    {
        let message_id = message.record.envelope.message_id;
        let attempt = derived_id(SUPERVISOR_INBOX_DOMAIN, &[message_id.as_bytes()]);
        if deliver(context, message_id, attempt).await? {
            acknowledge(
                context,
                &Participant::Supervisor,
                &[Delivery {
                    record: message.record.clone(),
                    note: None,
                    block: None,
                    outbox: vec![message_id],
                }],
                attempt,
            )
            .await?;
        }
    }
    Ok(())
}

/// Builds, records, and stages the bundle an answer carries to its asker.
pub(crate) async fn bundle_for_answer(
    context: &RunContext,
    source: &UnitState,
    asker: &UnitState,
    answer: &AdmittedMessage,
) -> Result<Option<ContextBundle>, WorkflowError> {
    let Some(latest) = source.latest.as_ref() else {
        return Ok(None);
    };
    let record = context.record().await?;
    let max_bytes = context
        .plan
        .specs
        .get(&asker.unit.task_key)
        .ok_or(WorkflowError::Internal)?
        .context_policy
        .max_bundle_bytes;
    let bundle_id = derived_id(
        BUNDLE_ID_DOMAIN,
        &[answer.record.envelope.message_id.as_bytes()],
    );
    let selections: Vec<FileSelection> = answer
        .record
        .envelope
        .source_refs
        .iter()
        .map(|reference| FileSelection {
            path: reference.path.clone(),
            start_line: reference.start_line,
            end_line: reference.end_line,
        })
        .collect();
    let claims = vec![AttributedClaim {
        attribution: Attribution::WorkerClaim {
            task_key: source.unit.task_key.clone(),
        },
        text: answer.body.clone(),
    }];
    let source_participant = source.unit.participant();
    let view = SnapshotView {
        task_key: &source.unit.task_key,
        source: &source_participant,
        snapshot_digest: latest.digest(),
        base_commit: &record.base_commit,
        files: &latest.texts,
    };
    let mut request = BundleRequest {
        bundle_id,
        workflow_id: context.workflow_id,
        request_message_id: answer.record.envelope.reply_to,
        selections: &selections,
        claims: &claims,
        audience: vec![asker.unit.participant(), Participant::Supervisor],
        max_bytes,
    };
    // An unusable selection or an over-budget excerpt degrades to the
    // attributed claim alone; nothing outside the snapshot is ever added.
    let bundle = match build_bundle(&request, view) {
        Ok(bundle) => bundle,
        Err(_) => {
            request.selections = &[];
            match build_bundle(&request, view) {
                Ok(bundle) => bundle,
                Err(_) => return Ok(None),
            }
        }
    };
    stage_bundle(context, &record, &bundle).await?;
    Ok(Some(bundle))
}

/// Keeps the bundle text in the outbox (and the opt-in content store),
/// then records the content-free bundle receipt.
async fn stage_bundle(
    context: &RunContext,
    record: &WorkflowRecord,
    bundle: &ContextBundle,
) -> Result<(), WorkflowError> {
    let text = render_bundle_text(bundle);
    let state = context.setup.state.clone();
    let workflow_id = context.workflow_id;
    let bundle_id = bundle.bundle_id;
    let staged = text.clone();
    tokio::task::spawn_blocking(move || state.put_outbox(workflow_id, bundle_id, &staged))
        .await
        .map_err(|_| WorkflowError::Internal)??;
    if let ContentStoreMode::Enabled { retention_days } = record.content_store {
        store_content(
            context,
            text.into_bytes(),
            ContentKind::Bundle,
            Some(bundle_id),
            retention_days,
        )
        .await?;
    }
    let mut stripped = bundle.clone();
    for excerpt in &mut stripped.excerpts {
        excerpt.text.clear();
    }
    for claim in &mut stripped.claims {
        claim.text.clear();
    }
    let writer = context.writer.clone();
    let receipt = WorkflowReceipt::Bundle(stripped);
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            receipt.clone(),
            None,
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(())
}

/// Writes one blob to the opt-in content store and indexes it.
pub(crate) async fn store_content(
    context: &RunContext,
    bytes: Vec<u8>,
    kind: ContentKind,
    bundle_id: Option<Uuid>,
    retention_days: u32,
) -> Result<Sha256Digest, WorkflowError> {
    let state = context.setup.state.clone();
    let workflow_id = context.workflow_id;
    let byte_count = bytes.len() as u64;
    let digest = tokio::task::spawn_blocking(move || {
        let digest = state.put_content(&bytes)?;
        state.add_content_index(ContentIndexEntry {
            sha256: digest,
            workflow_id,
            bundle_id,
        })?;
        Ok::<_, WorkflowError>(digest)
    })
    .await
    .map_err(|_| WorkflowError::Internal)??;
    let now = now_ms();
    let entry = ContentEntry {
        content_sha256: digest,
        workflow_id: Some(workflow_id),
        kind,
        byte_count,
        state: ContentState::Present,
        retain_until_ms: now.saturating_add(u64::from(retention_days).saturating_mul(DAY_MS)),
        updated_at_ms: now,
    };
    let writer = context.writer.clone();
    call_writer(move || writer.record_workflow_content(entry.clone(), OffsetDateTime::now_utc()))
        .await?;
    Ok(digest)
}

/// Admitted messages waiting for `recipient`, in recipient sequence order,
/// with their bodies and any bundle they carry.
pub(crate) async fn pending_deliveries(
    context: &RunContext,
    recipient: &UnitState,
) -> Result<Vec<Delivery>, WorkflowError> {
    let snapshot = context.snapshot().await?;
    let participant = recipient.unit.participant();
    let mut inbox: Vec<&MessageRecord> = snapshot
        .messages
        .iter()
        .filter(|message| message.envelope.recipient == participant)
        .collect();
    inbox.sort_by_key(|message| message.recipient_sequence);
    let bundles: BTreeMap<Uuid, &ContextBundle> = snapshot
        .receipts
        .iter()
        .filter_map(|receipt| match receipt {
            WorkflowReceipt::Bundle(bundle) => Some((bundle.bundle_id, bundle)),
            _ => None,
        })
        .collect();
    let mut cursor = InboxCursor::starting_after(0);
    let mut deliveries = Vec::new();
    for message in inbox {
        match cursor.offer(message.recipient_sequence, message.envelope.message_id) {
            InboxDecision::Accept => {}
            InboxDecision::Duplicate => continue,
            InboxDecision::OutOfOrder { .. } => break,
        }
        // A message delivered to an attempt that never completed stays
        // eligible; `deliver` refuses it for any attempt but that one.
        if !matches!(
            message.state,
            MessageState::Admitted | MessageState::Delivered
        ) || message.envelope.expires_at_ms <= now_ms()
        {
            continue;
        }
        let Ok(body) = read_body(context, message) else {
            continue;
        };
        let envelope = &message.envelope;
        let mut outbox = vec![envelope.message_id];
        let mut block = None;
        if envelope.kind == MessageKind::Answer {
            if let Some(bundle) = envelope.reply_to.and_then(|question| {
                bundles.values().find(|bundle| {
                    bundle.request_message_id == Some(question)
                        && bundle.audience.contains(&participant)
                })
            }) {
                if let Some(text) = read_bundle(context, bundle)? {
                    outbox.push(bundle.bundle_id);
                    block = Some(context_block(bundle, text));
                }
            }
        }
        if envelope.kind == MessageKind::ContextGrant {
            if let Some(bundle) = body
                .strip_prefix(GRANT_BODY_PREFIX)
                .and_then(|id| Uuid::parse_str(id.trim()).ok())
                .and_then(|id| bundles.get(&id))
            {
                if let Some(text) = read_stored_bundle(context, bundle)? {
                    block = Some(context_block(bundle, text));
                }
            }
        }
        deliveries.push(Delivery {
            record: message.clone(),
            note: Some(format!(
                "message_id: {}\nkind: {}\nfrom: {}\n{}",
                envelope.message_id,
                envelope.kind.as_str(),
                envelope.sender.label(),
                body
            )),
            block,
            outbox,
        });
    }
    Ok(deliveries)
}

fn context_block(bundle: &ContextBundle, text: String) -> ContextBlock {
    ContextBlock {
        bundle_id: bundle.bundle_id,
        bundle_digest: bundle.content_digest,
        source_label: bundle.source.label(),
        text,
    }
}

fn bundle_text_matches(bundle: &ContextBundle, bytes: &[u8]) -> bool {
    Sha256Digest::of_fields(BUNDLE_DIGEST_DOMAIN, &[bytes]) == bundle.content_digest
}

/// The staged text of a routed bundle, checked against its receipt.
fn read_bundle(
    context: &RunContext,
    bundle: &ContextBundle,
) -> Result<Option<String>, WorkflowError> {
    context
        .setup
        .state
        .read_outbox_checked(context.workflow_id, bundle.bundle_id, |bytes| {
            bundle_text_matches(bundle, bytes)
        })
}

/// A bundle's text from the opt-in content store, checked against its
/// receipt. `None` when the store is off or the text was deleted.
pub(crate) fn read_stored_bundle(
    context: &RunContext,
    bundle: &ContextBundle,
) -> Result<Option<String>, WorkflowError> {
    stored_bundle_text(&context.setup.state, context.workflow_id, bundle)
}

pub(crate) fn stored_bundle_text(
    state: &super::state_files::StateFiles,
    workflow_id: Uuid,
    bundle: &ContextBundle,
) -> Result<Option<String>, WorkflowError> {
    let Some(entry) = state.content_index()?.into_iter().find(|entry| {
        entry.workflow_id == workflow_id && entry.bundle_id == Some(bundle.bundle_id)
    }) else {
        return Ok(None);
    };
    match state.read_content(entry.sha256)? {
        Some(bytes) if bundle_text_matches(bundle, &bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| WorkflowError::ShareInvalid),
        Some(_) => Err(WorkflowError::ShareInvalid),
        None => Ok(None),
    }
}

/// The body of an operator context grant naming `bundle_id`.
#[must_use]
pub(crate) fn grant_body(bundle_id: Uuid) -> String {
    format!("{GRANT_BODY_PREFIX}{bundle_id}")
}
