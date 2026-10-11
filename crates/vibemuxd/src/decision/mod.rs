//! Daemon-side advisory decisions (ADR 034).
//!
//! [`DecisionService`] builds a typed `harness_route` question from the
//! pinned dispatch routes and the trusted probe, consults the external
//! backend only when the operator config and the request both allow it,
//! applies the deterministic fallback chain, and records a content-free
//! decision through the single writer. A decision grants nothing: a decided
//! dispatch re-runs every ADR 029 gate, and the store consumes the decision
//! in the admission transaction.

mod config_loader;
mod facts_builder;
mod peer_process;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use thiserror::Error;
use time::OffsetDateTime;
use tokio::{sync::Semaphore, time::timeout};
use uuid::Uuid;
use vibemux_decision::{
    DecisionError, DecisionKind, DecisionQuestion, DecisionRecord, DecisionStatus, ExternalError,
    ExternalResult, NewDecision, PeerRequest, PromptDigest, RouteFacts, Sha256Digest,
    plan_external, resolve_route,
};
use vibemux_harness::dispatch::{
    DispatchRequest, request::DISPATCH_REQUEST_SCHEMA_VERSION, request::MAX_PROMPT_BYTES,
};
use vibemux_store::{MAX_DECISION_PAGE, StoredDecision};

use crate::{
    WriterError, WriterHandle,
    harness_dispatch::{
        DispatchReceipt, DispatchServiceError, HarnessDispatchService, call_writer,
    },
};

pub use config_loader::{
    DecisionConfigLoadError, LoadedDecisionConfig, decision_config_path, load_decision_config,
};
pub use facts_builder::{eligible_harnesses, project_default};
pub use peer_process::{DECISION_PEER_NAME, PEER_DEADLINE, PeerCredential, bundled_peer, run_peer};

/// Where the service finds its inputs.
#[derive(Clone)]
pub struct DecisionServiceSettings {
    pub config_path: PathBuf,
    /// `None` resolves [`DECISION_PEER_NAME`] next to the running executable.
    pub peer: Option<PathBuf>,
    /// Bound on one peer exchange; [`PEER_DEADLINE`] in production.
    pub peer_deadline: Duration,
}

impl DecisionServiceSettings {
    /// Production layout: the config lives in the daemon state directory.
    #[must_use]
    pub fn for_project(state_dir: &Path) -> Self {
        Self {
            config_path: decision_config_path(state_dir),
            peer: None,
            peer_deadline: PEER_DEADLINE,
        }
    }
}

impl std::fmt::Debug for DecisionServiceSettings {
    /// Redacted: paths never reach a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecisionServiceSettings")
            .field("peer_deadline", &self.peer_deadline)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DecisionServiceError {
    #[error("{0}")]
    Decision(DecisionError),
    #[error("{0}")]
    Config(DecisionConfigLoadError),
    #[error("{0}")]
    Dispatch(DispatchServiceError),
    #[error("{0}")]
    Writer(WriterError),
}

impl DecisionServiceError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Decision(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Dispatch(error) => error.code(),
            Self::Writer(error) => error.code(),
        }
    }
}

impl From<DecisionError> for DecisionServiceError {
    fn from(error: DecisionError) -> Self {
        Self::Decision(error)
    }
}

impl From<DispatchServiceError> for DecisionServiceError {
    fn from(error: DispatchServiceError) -> Self {
        Self::Dispatch(error)
    }
}

impl From<WriterError> for DecisionServiceError {
    fn from(error: WriterError) -> Self {
        Self::Writer(error)
    }
}

/// One `harness_route` request. The prompt is hashed and measured here and
/// never leaves the daemon; only allowlisted labels and a size bucket reach
/// a backend.
#[derive(Clone)]
pub struct AdviseRequest {
    pub prompt: String,
    pub labels: Vec<String>,
    /// The per-request external consent; the config must allow it too.
    pub allow_external: bool,
}

impl std::fmt::Debug for AdviseRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdviseRequest")
            .field("prompt_bytes", &self.prompt.len())
            .field("labels", &self.labels.len())
            .field("allow_external", &self.allow_external)
            .finish()
    }
}

/// Cloneable handle; clones share one service.
#[derive(Clone)]
pub struct DecisionService {
    inner: Arc<ServiceInner>,
}

impl std::fmt::Debug for DecisionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecisionService")
            .finish_non_exhaustive()
    }
}

struct ServiceInner {
    writer: WriterHandle,
    dispatch: HarnessDispatchService,
    setup: Result<DecisionSetup, DecisionConfigLoadError>,
    /// One external exchange at a time.
    external: Semaphore,
}

/// Inputs fixed at startup.
struct DecisionSetup {
    config: LoadedDecisionConfig,
    peer: Option<PathBuf>,
    peer_deadline: Duration,
}

impl DecisionService {
    /// Loads and pins the decision config. Never fails: a missing or invalid
    /// config leaves `advise` unavailable with that code, while listing,
    /// showing, and dispatching recorded decisions still work.
    pub async fn start(
        writer: WriterHandle,
        dispatch: HarnessDispatchService,
        settings: DecisionServiceSettings,
    ) -> Self {
        let setup = tokio::task::spawn_blocking(move || prepare(&settings))
            .await
            .unwrap_or(Err(DecisionConfigLoadError::Decision(
                DecisionError::Internal,
            )));
        Self::with_setup(writer, dispatch, setup)
    }

    /// A service without a config, for daemons started without project
    /// paths.
    #[must_use]
    pub fn unconfigured(writer: WriterHandle, dispatch: HarnessDispatchService) -> Self {
        Self::with_setup(
            writer,
            dispatch,
            Err(DecisionConfigLoadError::Decision(
                DecisionError::NotConfigured,
            )),
        )
    }

    fn with_setup(
        writer: WriterHandle,
        dispatch: HarnessDispatchService,
        setup: Result<DecisionSetup, DecisionConfigLoadError>,
    ) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                writer,
                dispatch,
                setup,
                external: Semaphore::new(1),
            }),
        }
    }

    /// Decides which harness should run `request.prompt` and records the
    /// decision. Rejections (unknown label, no eligible harness, invalid
    /// prompt) write nothing; a backend failure falls back and is recorded
    /// as evidence.
    pub async fn advise(
        &self,
        request: AdviseRequest,
    ) -> Result<StoredDecision, DecisionServiceError> {
        let setup = self
            .inner
            .setup
            .as_ref()
            .map_err(|error| DecisionServiceError::Config(*error))?;
        let config = &setup.config.config;
        if request.prompt.trim().is_empty() || request.prompt.len() > MAX_PROMPT_BYTES {
            return Err(DecisionError::InvalidRequest.into());
        }
        let prompt = PromptDigest {
            sha256: Sha256Digest::of(request.prompt.as_bytes()),
            byte_count: request.prompt.len() as u64,
        };
        let labels = config.check_labels(&request.labels)?;
        let catalog = self.inner.dispatch.catalog()?;
        let dispatch_config_sha256 = self.inner.dispatch.config_digest()?;
        let writer = self.inner.writer.clone();
        let rows = call_writer(move || writer.harness_rows()).await?;
        let eligible = eligible_harnesses(&catalog, &rows);
        let default = project_default(&rows);
        let threshold = config.harness_route_threshold()?;
        let question =
            DecisionQuestion::harness_route(&eligible, &config.option_descriptions, threshold)?;
        let facts = RouteFacts::new(labels.clone(), prompt.byte_count, default, &eligible);
        let external =
            match plan_external(eligible.len(), request.allow_external, config.jev.enabled) {
                Some(reason) => ExternalResult::Skipped(reason),
                None => match self.consult(setup, &question, &facts).await {
                    Ok(answer) => ExternalResult::Answered(answer),
                    Err(error) => ExternalResult::Failed(error),
                },
            };
        let resolution = resolve_route(
            &question,
            &eligible,
            &labels,
            default,
            &config.local_rules,
            external,
        );
        let record = DecisionRecord::new(
            NewDecision {
                decision_id: Uuid::new_v4(),
                kind: DecisionKind::HarnessRoute,
                options: question.option_ids(),
                labels,
                prompt,
                project_default: default,
                threshold,
                dispatch_config_sha256,
                decision_config_sha256: setup.config.digest,
                created_at: OffsetDateTime::now_utc(),
            },
            resolution,
        );
        let writer = self.inner.writer.clone();
        // Safe to repeat: recording the same record again writes nothing.
        let commit = call_writer(move || writer.record_decision(record.clone())).await?;
        Ok(commit.decision)
    }

    /// The recorded decision, or `decision_not_found`.
    pub async fn show(&self, decision_id: Uuid) -> Result<StoredDecision, DecisionServiceError> {
        let writer = self.inner.writer.clone();
        call_writer(move || writer.decision(decision_id))
            .await?
            .ok_or(DecisionServiceError::Decision(DecisionError::NotFound))
    }

    /// Up to `limit` decisions created after `after_sequence`, oldest first.
    pub async fn list(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<StoredDecision>, DecisionServiceError> {
        if !(1..=MAX_DECISION_PAGE).contains(&limit) {
            return Err(DecisionError::InvalidRequest.into());
        }
        let writer = self.inner.writer.clone();
        Ok(call_writer(move || writer.decisions_after(after_sequence, limit)).await?)
    }

    /// Submits `prompt` to the harness the decision chose. The dispatch
    /// service re-runs every admission gate; the store checks the decision
    /// binding and consumes the decision in the admission transaction.
    pub async fn dispatch(
        &self,
        decision_id: Uuid,
        request_id: Uuid,
        prompt: String,
    ) -> Result<DispatchReceipt, DecisionServiceError> {
        let stored = self.show(decision_id).await?;
        let harness = match (stored.record.status, stored.record.chosen_harness) {
            (DecisionStatus::Resolved, Some(harness)) => harness,
            (DecisionStatus::Escalated, _) => return Err(DecisionError::Escalated.into()),
            (DecisionStatus::Resolved, None) => return Err(DecisionError::Internal.into()),
        };
        let request = DispatchRequest {
            schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
            request_id,
            harness,
            prompt,
        };
        Ok(self
            .inner
            .dispatch
            .submit_decided(decision_id, request)
            .await?)
    }

    /// One external exchange: both consent gates already passed. The
    /// credential is read only here, and a missing one never spawns a peer.
    async fn consult(
        &self,
        setup: &DecisionSetup,
        question: &DecisionQuestion,
        facts: &RouteFacts,
    ) -> Result<vibemux_decision::ValidatedAnswer, ExternalError> {
        let jev = &setup.config.config.jev;
        let credential = PeerCredential::from_environment(&jev.credential_env)?;
        let executable = setup.peer.as_ref().ok_or(ExternalError::PeerUnavailable)?;
        let request = PeerRequest::for_route(question, facts, jev);
        let _permit = timeout(setup.peer_deadline, self.inner.external.acquire())
            .await
            .map_err(|_| ExternalError::PeerTimeout)?
            .map_err(|_| ExternalError::PeerUnavailable)?;
        let answer = run_peer(executable, &request, credential, setup.peer_deadline).await?;
        answer.validate(question, &jev.pinned_model)
    }
}

fn prepare(settings: &DecisionServiceSettings) -> Result<DecisionSetup, DecisionConfigLoadError> {
    let config = load_decision_config(&settings.config_path)?;
    let peer = match &settings.peer {
        Some(path) => Some(path.clone()),
        None => bundled_peer(),
    };
    Ok(DecisionSetup {
        config,
        peer: peer.filter(|path| path.is_file()),
        peer_deadline: settings.peer_deadline,
    })
}
