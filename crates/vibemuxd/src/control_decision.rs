//! Control v8 decision operations (ADR 034 §11).
//!
//! Every argument is a strict JSON document; a malformed one is
//! `control_invalid_request`, and a service refusal passes its `decision_*`,
//! `harness_dispatch_*`, `writer_*`, or `store_*` code through. Decision
//! views are allowlisted: they carry no digest that identifies the prompt or
//! a config. A decided submission answers with the ordinary dispatch receipt.

use time::OffsetDateTime;
use vibemux_decision::{
    DecisionKind, DecisionSource, DecisionStatus, ExternalEvidence, Label, OptionId,
    PromptSizeBucket, Threshold,
};
use vibemux_harness::AgentKind;
use vibemux_store::{MAX_DECISION_PAGE, StoredDecision};

use super::*;
use crate::decision::{AdviseRequest, DecisionService, DecisionServiceError, PEER_DEADLINE};

/// Client deadline of an advice: the peer deadline plus time for the
/// writer and the frame.
pub const DECISION_ADVISE_DEADLINE: Duration = Duration::from_secs(PEER_DEADLINE.as_secs() + 20);
/// Bound on the lookup and list arguments.
const MAX_LOOKUP_ARGUMENT_BYTES: usize = 1024;
/// Headroom for the response envelope around a payload.
const RESPONSE_ENVELOPE_BYTES: usize = 1024;
/// Encoded budget of one decision page.
const PAGE_ENCODED_BYTES: usize = MAX_CONTROL_FRAME_BYTES - 2 * RESPONSE_ENVELOPE_BYTES;

/// Argument of `decision_advise`. The prompt is hashed and measured in the
/// daemon and never reaches a backend, a record, or `Debug`.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAdviseArgument {
    pub prompt: String,
    #[serde(default)]
    pub labels: Vec<String>,
    /// The per-request consent to an external backend; the operator config
    /// must enable it too.
    #[serde(default)]
    pub allow_external: bool,
}

impl fmt::Debug for DecisionAdviseArgument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecisionAdviseArgument")
            .field("prompt_bytes", &self.prompt.len())
            .field("labels", &self.labels.len())
            .field("allow_external", &self.allow_external)
            .finish()
    }
}

/// Argument of `decision_show`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionLookup {
    pub decision_id: Uuid,
}

/// Argument of `decision_list`: up to `limit` decisions created after
/// `after_sequence`, oldest first.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionListQuery {
    pub after_sequence: u64,
    pub limit: usize,
}

/// Argument of `decision_dispatch_submit`: run `prompt` on the harness the
/// decision chose. The prompt must be the one the decision was made for.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionDispatchArgument {
    pub decision_id: Uuid,
    pub request_id: Uuid,
    pub prompt: String,
}

impl fmt::Debug for DecisionDispatchArgument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecisionDispatchArgument")
            .field("decision_id", &self.decision_id)
            .field("request_id", &self.request_id)
            .field("prompt_bytes", &self.prompt.len())
            .finish()
    }
}

/// Allowlisted view of a decision: everything but the prompt and config
/// digests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionView {
    pub decision_id: Uuid,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub chosen_harness: Option<AgentKind>,
    pub source: DecisionSource,
    pub rule_index: Option<u32>,
    pub options: Vec<OptionId>,
    pub labels: Vec<Label>,
    pub prompt_size: PromptSizeBucket,
    pub prompt_bytes: u64,
    pub project_default: Option<AgentKind>,
    pub threshold_millionths: Threshold,
    pub external: ExternalEvidence,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub consumed_request_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub consumed_at: Option<OffsetDateTime>,
    /// Sequence of the event that recorded the decision; the list cursor.
    pub created_sequence: u64,
    /// Sequence of the event that last changed the decision.
    pub updated_sequence: u64,
}

impl From<StoredDecision> for DecisionView {
    fn from(stored: StoredDecision) -> Self {
        let record = stored.record;
        Self {
            decision_id: record.decision_id,
            kind: record.kind,
            status: record.status,
            chosen_harness: record.chosen_harness,
            source: record.source,
            rule_index: record.rule_index,
            options: record.options,
            labels: record.labels,
            prompt_size: record.prompt_size,
            prompt_bytes: record.prompt_bytes,
            project_default: record.project_default,
            threshold_millionths: record.threshold_millionths,
            external: record.external,
            created_at: record.created_at,
            expires_at: record.expires_at,
            consumed_request_id: record
                .consumption
                .as_ref()
                .map(|consumption| consumption.request_id),
            consumed_at: record
                .consumption
                .as_ref()
                .map(|consumption| consumption.consumed_at),
            created_sequence: stored.created_sequence,
            updated_sequence: stored.updated_sequence,
        }
    }
}

/// One page of decisions. `next_after_sequence` is the cursor of the next
/// page, absent once the listing is exhausted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionPage {
    pub decisions: Vec<DecisionView>,
    pub next_after_sequence: Option<u64>,
}

impl ControlClient {
    /// Decides which harness should run `argument.prompt` and records it.
    /// Waits up to [`DECISION_ADVISE_DEADLINE`] since it may consult the
    /// external peer.
    pub async fn decision_advise(
        &self,
        argument: &DecisionAdviseArgument,
    ) -> Result<DecisionView, ControlError> {
        match self
            .decision_request(
                ControlOperation::DecisionAdvise,
                argument,
                DECISION_ADVISE_DEADLINE,
            )
            .await?
        {
            ControlPayload::Decision(view) => Ok(*view),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn decision_show(&self, decision_id: Uuid) -> Result<DecisionView, ControlError> {
        match self
            .decision_request(
                ControlOperation::DecisionShow,
                &DecisionLookup { decision_id },
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::Decision(view) => Ok(*view),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn decision_list(
        &self,
        query: DecisionListQuery,
    ) -> Result<DecisionPage, ControlError> {
        match self
            .decision_request(ControlOperation::DecisionList, &query, CONTROL_DEADLINE)
            .await?
        {
            ControlPayload::DecisionPage(page) => Ok(page),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    /// Admits the decided request; returns once the attempt is admitted.
    pub async fn decision_dispatch_submit(
        &self,
        argument: &DecisionDispatchArgument,
    ) -> Result<HarnessDispatchReceipt, ControlError> {
        match self
            .decision_request(
                ControlOperation::DecisionDispatchSubmit,
                argument,
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::HarnessDispatchReceipt(receipt) => Ok(receipt),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    async fn decision_request(
        &self,
        operation: ControlOperation,
        value: &impl Serialize,
        deadline: Duration,
    ) -> Result<ControlPayload, ControlError> {
        self.require_operation(operation.clone())?;
        let encoded = serde_json::to_string(value).map_err(|_| ControlError::InvalidRequest)?;
        self.request_with_argument(operation, Some(encoded), deadline)
            .await
    }
}

pub(super) async fn dispatch(
    service: &DecisionService,
    operation: ControlOperation,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let result = match operation {
        ControlOperation::DecisionAdvise => {
            // The frame bounds the argument; the service bounds the prompt.
            let argument: DecisionAdviseArgument = parse_bounded(argument, None)?;
            service
                .advise(AdviseRequest {
                    prompt: argument.prompt,
                    labels: argument.labels,
                    allow_external: argument.allow_external,
                })
                .await
                .map(|stored| ControlPayload::Decision(Box::new(stored.into())))
        }
        ControlOperation::DecisionShow => {
            let lookup: DecisionLookup = parse_bounded(argument, Some(MAX_LOOKUP_ARGUMENT_BYTES))?;
            service
                .show(lookup.decision_id)
                .await
                .map(|stored| ControlPayload::Decision(Box::new(stored.into())))
        }
        ControlOperation::DecisionList => {
            let query: DecisionListQuery =
                parse_bounded(argument, Some(MAX_LOOKUP_ARGUMENT_BYTES))?;
            service
                .list(query.after_sequence, query.limit)
                .await
                .and_then(|stored| {
                    fit_page(stored, query.limit)
                        .map(ControlPayload::DecisionPage)
                        .ok_or(DecisionServiceError::Decision(
                            vibemux_decision::DecisionError::Internal,
                        ))
                })
        }
        ControlOperation::DecisionDispatchSubmit => {
            let argument: DecisionDispatchArgument = parse_bounded(argument, None)?;
            service
                .dispatch(argument.decision_id, argument.request_id, argument.prompt)
                .await
                .map(|receipt| ControlPayload::HarnessDispatchReceipt(receipt.into()))
        }
        _ => return Err(ControlError::InvalidRequest),
    };
    let payload = result.map_err(|error| ControlError::Remote {
        code: error.code().to_string(),
    })?;
    // Pages are sized to fit; this catches any other oversized payload with
    // a typed error instead of a dropped frame.
    if encoded_len(&payload)? > MAX_CONTROL_FRAME_BYTES - RESPONSE_ENVELOPE_BYTES {
        return Err(ControlError::FrameTooLarge);
    }
    Ok(payload)
}

/// The longest prefix of `stored` whose page fits [`PAGE_ENCODED_BYTES`].
/// The cursor points past the last decision kept whenever the page was full
/// or shortened. `None` if not even one decision fits.
fn fit_page(stored: Vec<StoredDecision>, limit: usize) -> Option<DecisionPage> {
    let full = stored.len() >= limit.min(MAX_DECISION_PAGE);
    let mut decisions: Vec<DecisionView> = stored.into_iter().map(DecisionView::from).collect();
    let mut shortened = false;
    loop {
        let next_after_sequence = (full || shortened)
            .then(|| decisions.last().map(|view| view.created_sequence))
            .flatten();
        let page = DecisionPage {
            decisions,
            next_after_sequence,
        };
        let fits = serde_json::to_vec(&page)
            .ok()
            .is_some_and(|bytes| bytes.len() <= PAGE_ENCODED_BYTES);
        if fits {
            return Some(page);
        }
        decisions = page.decisions;
        if decisions.len() <= 1 {
            return None;
        }
        decisions.pop();
        shortened = true;
    }
}

fn encoded_len(payload: &ControlPayload) -> Result<usize, ControlError> {
    serde_json::to_vec(payload)
        .map(|bytes| bytes.len())
        .map_err(|_| ControlError::InvalidFrame)
}

/// Parses a required argument, refusing one above `limit` when given.
fn parse_bounded<T: DeserializeOwned>(
    argument: Option<String>,
    limit: Option<usize>,
) -> Result<T, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if limit.is_some_and(|limit| argument.len() > limit) {
        return Err(ControlError::InvalidRequest);
    }
    serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)
}

#[cfg(test)]
mod tests {
    use vibemux_decision::{
        DecisionBackend, DecisionConsumption, DecisionRecord, ExternalError, ExternalStatus,
        NewDecision, PromptDigest, RouteResolution, Sha256Digest,
    };
    use vibemux_harness::dispatch::request::MAX_PROMPT_BYTES;

    use super::*;

    const OPERATIONS: [ControlOperation; 4] = [
        ControlOperation::DecisionAdvise,
        ControlOperation::DecisionList,
        ControlOperation::DecisionShow,
        ControlOperation::DecisionDispatchSubmit,
    ];

    /// The largest decision a valid config can produce: every harness
    /// eligible, the label limit reached, and full external evidence.
    fn stored(sequence: u64) -> StoredDecision {
        let options: Vec<OptionId> = AgentKind::all()
            .into_iter()
            .map(|harness| OptionId::new(harness.command_name()).expect("option"))
            .chain([OptionId::escalate()])
            .collect();
        let labels: Vec<Label> = (0..vibemux_decision::config::MAX_CONFIG_LABELS)
            .map(|index| Label::new(format!("l{index:0>31}")).expect("label"))
            .collect();
        let resolution = RouteResolution {
            status: DecisionStatus::Resolved,
            chosen_harness: Some(AgentKind::Codex),
            source: DecisionSource::LocalRule,
            rule_index: Some(u32::MAX),
            external: ExternalEvidence {
                status: ExternalStatus::Failed,
                backend: Some(DecisionBackend::Jev),
                error: Some(ExternalError::PeerTimeout),
                answer: None,
            },
        };
        let mut record = DecisionRecord::new(
            NewDecision {
                decision_id: Uuid::new_v4(),
                kind: DecisionKind::HarnessRoute,
                options,
                labels,
                prompt: PromptDigest {
                    sha256: Sha256Digest::of(b"prompt"),
                    byte_count: MAX_PROMPT_BYTES as u64,
                },
                project_default: Some(AgentKind::Claude),
                threshold: Threshold::from_unit(0.9).expect("threshold"),
                dispatch_config_sha256: Sha256Digest::of(b"dispatch"),
                decision_config_sha256: Sha256Digest::of(b"decision"),
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            resolution,
        );
        record.consumption = Some(DecisionConsumption {
            request_id: Uuid::new_v4(),
            consumed_at: OffsetDateTime::UNIX_EPOCH,
        });
        StoredDecision {
            record,
            created_sequence: sequence,
            updated_sequence: u64::MAX,
        }
    }

    #[test]
    fn the_decision_operations_require_v8_and_keep_v7_compatible() {
        for operation in OPERATIONS {
            assert_eq!(operation.minimum_protocol_version(), CONTROL_PROTOCOL_V8);
        }
        assert_eq!(CONTROL_PROTOCOL_VERSION, CONTROL_PROTOCOL_V8);
        assert!(supported_control_version(CONTROL_PROTOCOL_V7));
        assert!(supported_control_version(CONTROL_PROTOCOL_V8));
        assert!(!supported_control_version(CONTROL_PROTOCOL_V8 + 1));
        assert!(DECISION_ADVISE_DEADLINE > PEER_DEADLINE);
    }

    #[test]
    fn decision_codes_pass_through_as_remote_errors() {
        for code in [
            "decision_not_configured",
            "decision_already_consumed",
            "decision_config_malformed",
        ] {
            assert_eq!(
                remote_error(code.to_string()),
                ControlError::Remote {
                    code: code.to_string()
                }
            );
        }
    }

    #[test]
    fn a_view_carries_no_digest() {
        let view = DecisionView::from(stored(7));
        let encoded = serde_json::to_string(&view).expect("encode");
        for digest in ["prompt", "dispatch", "decision"] {
            let hex = Sha256Digest::of(digest.as_bytes()).to_string();
            assert!(
                !encoded.contains(&hex),
                "the view holds the {digest} digest"
            );
        }
        assert!(!encoded.contains("sha256"));
        assert_eq!(view.created_sequence, 7);
        assert!(view.consumed_request_id.is_some());
    }

    #[test]
    fn a_full_page_of_the_largest_decisions_fits_the_frame() {
        let largest: Vec<StoredDecision> = (1..=MAX_DECISION_PAGE as u64).map(stored).collect();
        let page = fit_page(largest, MAX_DECISION_PAGE).expect("page");
        assert!(!page.decisions.is_empty());
        let last = page.decisions.last().expect("last").created_sequence;
        assert_eq!(page.next_after_sequence, Some(last));
        let response =
            ControlResponse::success("r".repeat(128), ControlPayload::DecisionPage(page.clone()));
        assert!(serde_json::to_vec(&response).expect("encode").len() < MAX_CONTROL_FRAME_BYTES);

        let short = fit_page(vec![stored(1)], 10).expect("short page");
        assert_eq!(short.next_after_sequence, None);
    }

    #[test]
    fn arguments_are_strict_and_prompts_stay_out_of_debug() {
        for argument in [
            None,
            Some(r#"{"decision_id":"not-a-uuid"}"#.to_string()),
            Some(format!(r#"{{"decision_id":"{}","extra":1}}"#, Uuid::nil())),
            Some("x".repeat(MAX_LOOKUP_ARGUMENT_BYTES + 1)),
        ] {
            assert_eq!(
                parse_bounded::<DecisionLookup>(argument, Some(MAX_LOOKUP_ARGUMENT_BYTES)),
                Err(ControlError::InvalidRequest)
            );
        }
        let advise = DecisionAdviseArgument {
            prompt: "a very secret prompt body".to_string(),
            labels: vec!["rust".to_string()],
            allow_external: true,
        };
        let dispatch = DecisionDispatchArgument {
            decision_id: Uuid::nil(),
            request_id: Uuid::nil(),
            prompt: "a very secret prompt body".to_string(),
        };
        for (operation, argument) in [
            (
                ControlOperation::DecisionAdvise,
                serde_json::to_string(&advise).expect("encode"),
            ),
            (
                ControlOperation::DecisionDispatchSubmit,
                serde_json::to_string(&dispatch).expect("encode"),
            ),
        ] {
            let control = ControlRequest {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: "r".repeat(128),
                token: "t".repeat(64),
                operation,
                argument: Some(argument),
            };
            assert!(!format!("{control:?}").contains("secret"));
        }
        assert!(!format!("{advise:?}").contains("secret"));
        assert!(!format!("{dispatch:?}").contains("secret"));
        // The largest prompt still fits one frame once encoded.
        let largest = DecisionAdviseArgument {
            prompt: "p".repeat(MAX_PROMPT_BYTES),
            ..advise
        };
        let control = ControlRequest {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: "r".repeat(128),
            token: "t".repeat(64),
            operation: ControlOperation::DecisionAdvise,
            argument: Some(serde_json::to_string(&largest).expect("encode")),
        };
        assert!(serde_json::to_vec(&control).expect("encode").len() < MAX_CONTROL_FRAME_BYTES);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn decisions_fail_closed_without_a_config_and_require_v8() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = DaemonControlServer::start(&temp.path().join("state.sqlite3"), temp.path())
            .await
            .expect("server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let remote = |code: &str| ControlError::Remote {
            code: code.to_string(),
        };
        let advise = DecisionAdviseArgument {
            prompt: "must not be decided".to_string(),
            labels: Vec::new(),
            allow_external: true,
        };
        assert_eq!(
            client.decision_advise(&advise).await,
            Err(remote("decision_not_configured"))
        );
        // Recorded decisions stay readable without a config.
        let empty = client
            .decision_list(DecisionListQuery {
                after_sequence: 0,
                limit: 10,
            })
            .await
            .expect("list");
        assert!(empty.decisions.is_empty());
        assert_eq!(empty.next_after_sequence, None);
        assert_eq!(
            client
                .decision_list(DecisionListQuery {
                    after_sequence: 0,
                    limit: MAX_DECISION_PAGE + 1,
                })
                .await
                .map(|_| ()),
            Err(remote("decision_invalid_request"))
        );
        assert_eq!(
            client.decision_show(Uuid::new_v4()).await,
            Err(remote("decision_not_found"))
        );
        let submit = DecisionDispatchArgument {
            decision_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            prompt: "must not run".to_string(),
        };
        assert_eq!(
            client.decision_dispatch_submit(&submit).await,
            Err(remote("decision_not_found"))
        );
        // A v7 client refuses the v8 operations before they reach the server.
        let mut v7 = client.clone();
        v7.descriptor.version = CONTROL_PROTOCOL_V7;
        assert_eq!(
            v7.decision_show(Uuid::new_v4()).await,
            Err(ControlError::UnsupportedVersion)
        );
        assert!(v7.health().await.expect("v7 health").healthy);
        server.shutdown().await.expect("shutdown");
    }
}
