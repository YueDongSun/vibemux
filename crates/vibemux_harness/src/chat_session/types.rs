//! Public contracts for the pure persistent session engine.

use std::{fmt, path::Path};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_types::ToolName;
use vibemux_types::chat_tool::ToolRecord;

/// Qualification inputs inspected on 2026-10-31; fixtures are not live proof.
pub const CODEX_SCHEMA_VERSION: &str = "0.166.0";
pub const CLAUDE_HELP_VERSION: &str = "2.1.304";

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;
pub const MAX_REPLY_BYTES: usize = 256 * 1024;
pub const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_MODEL_BYTES: usize = 256;
const MAX_DIRECTORY_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionProtocol {
    ClaudeStreamJson,
    CodexAppServer,
}

/// Only the daemon may construct this from its own confirmed session mapping.
/// A vendor ID supplied by a renderer is not an authorized resume target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTarget {
    /// The launch owner pins this UUID with --session-id or exact --resume.
    Claude { session_id: Uuid, resume: bool },
    /// None starts a new root thread; Some resumes only the exact owned thread.
    Codex { resume_thread_id: Option<String> },
}

impl SessionTarget {
    #[must_use]
    pub const fn protocol(&self) -> SessionProtocol {
        match self {
            Self::Claude { .. } => SessionProtocol::ClaudeStreamJson,
            Self::Codex { .. } => SessionProtocol::CodexAppServer,
        }
    }

    #[must_use]
    pub fn expected_id(&self) -> Option<String> {
        match self {
            Self::Claude { session_id, .. } => Some(session_id.to_string()),
            Self::Codex { resume_thread_id } => resume_thread_id.clone(),
        }
    }
}

#[derive(Clone)]
pub struct SessionConfig {
    pub target: SessionTarget,
    /// A fresh process-generation nonce, supplied by its daemon owner.
    pub generation_id: Uuid,
    /// Canonical absolute UTF-8 cwd verified by the daemon, never the renderer.
    pub working_directory: String,
    pub model: Option<String>,
}

impl fmt::Debug for SessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionConfig")
            .field("protocol", &self.target.protocol())
            .field("generation_id", &self.generation_id)
            .finish_non_exhaustive()
    }
}

impl SessionConfig {
    pub(super) fn validate(&self) -> Result<(), SessionError> {
        if self.generation_id.is_nil()
            || self.working_directory.is_empty()
            || self.working_directory.len() > MAX_DIRECTORY_BYTES
            || self.working_directory.chars().any(char::is_control)
            || !Path::new(&self.working_directory).is_absolute()
            || self.model.as_ref().is_some_and(|model| {
                model.trim().is_empty()
                    || model.len() > MAX_MODEL_BYTES
                    || model.chars().any(char::is_control)
            })
        {
            return Err(SessionError::InvalidConfiguration);
        }
        match &self.target {
            SessionTarget::Claude { session_id, .. } if session_id.is_nil() => {
                Err(SessionError::InvalidConfiguration)
            }
            SessionTarget::Codex {
                resume_thread_id: Some(id),
            } => validate_identifier(id),
            _ => Ok(()),
        }
    }
}

/// These are protocol/observer bounds, not inference or resource allowances.
/// Callers may lower the defaults; widening a hard bound needs a reviewed change.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLimits {
    pub max_frame_bytes: usize,
    pub max_prompt_bytes: usize,
    pub max_reply_bytes: usize,
    pub max_turns: usize,
    pub max_records: usize,
    pub max_record_bytes: usize,
    pub max_rpc_requests: usize,
    pub max_pending_frames: usize,
    pub max_pending_bytes: usize,
    pub max_text_items: usize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: MAX_FRAME_BYTES,
            max_prompt_bytes: MAX_PROMPT_BYTES,
            max_reply_bytes: MAX_REPLY_BYTES,
            max_turns: 1024,
            max_records: 65_536,
            max_record_bytes: 32 * 1024 * 1024,
            max_rpc_requests: 4096,
            max_pending_frames: 64,
            max_pending_bytes: MAX_FRAME_BYTES,
            max_text_items: 1024,
        }
    }
}

impl SessionLimits {
    pub(super) fn validate(&self) -> Result<(), SessionError> {
        let ceiling = Self::default();
        for (actual, maximum) in [
            (self.max_frame_bytes, ceiling.max_frame_bytes),
            (self.max_prompt_bytes, ceiling.max_prompt_bytes),
            (self.max_reply_bytes, ceiling.max_reply_bytes),
            (self.max_turns, ceiling.max_turns),
            (self.max_records, ceiling.max_records),
            (self.max_record_bytes, ceiling.max_record_bytes),
            (self.max_rpc_requests, ceiling.max_rpc_requests),
            (self.max_pending_frames, ceiling.max_pending_frames),
            (self.max_pending_bytes, ceiling.max_pending_bytes),
            (self.max_text_items, ceiling.max_text_items),
        ] {
            if actual == 0 || actual > maximum {
                return Err(SessionError::InvalidConfiguration);
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct TurnRequest {
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub prompt: String,
}

impl fmt::Debug for TurnRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TurnRequest")
            .field("request_id", &self.request_id)
            .field("turn_id", &self.turn_id)
            .field("prompt_bytes", &self.prompt.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    New,
    Initializing,
    Opening,
    Ready,
    Busy,
    Closing,
    Closed,
    Suspended,
    RecoveryRequired,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    Prepared,
    Running,
    Cancelling,
    Finished,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnSummary {
    pub request_id: Uuid,
    pub turn_id: Uuid,
    /// Codex's correlated turn ID. Claude uses the input request UUID echo.
    pub vendor_turn_id: Option<String>,
    pub phase: TurnPhase,
    pub outcome: Option<TurnOutcome>,
    pub acknowledged: bool,
    pub cancel_requested: bool,
    pub prompt_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSnapshot {
    pub protocol: SessionProtocol,
    pub generation_id: Uuid,
    pub phase: SessionPhase,
    pub expected_vendor_session_id: Option<String>,
    pub vendor_session_id: Option<String>,
    pub vendor_session_confirmed: bool,
    pub active_turn: Option<TurnSummary>,
    pub last_turn: Option<TurnSummary>,
    pub turns_started: usize,
    pub records_received: u64,
    pub record_bytes: usize,
    pub rpc_requests: usize,
    pub close_requested: bool,
    pub error: Option<SessionError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IgnoredReason {
    Duplicate,
    RetiredTurn,
    Nested,
    Historical,
    UnknownNotification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeniedCapability {
    Command,
    FileChange,
    Permissions,
    UserInput,
    Tool,
    Mcp,
    Unknown,
}

/// Content-free protocol facts, mapped explicitly by the daemon to its events.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    Initialized,
    SessionBound { vendor_session_id: String },
    TurnPrepared { turn: TurnSummary },
    TurnAcknowledged { turn: TurnSummary },
    CancellationRequested { turn: TurnSummary },
    CancellationAcknowledged { turn_id: Uuid },
    CancellationUnconfirmed { turn_id: Uuid },
    TurnFinished { turn: TurnSummary },
    TurnUncertain { turn: TurnSummary },
    Closing,
    Closed,
    Suspended,
    CapabilityDenied { capability: DeniedCapability },
    RecordIgnored { reason: IgnoredReason },
}

/// Complete replacement display snapshot. Never append it as a delta.
/// Deliberately not Serialize; private transcript publication is an explicit
/// daemon writer decision, not an ordinary event/log operation.
#[derive(Clone, Eq, PartialEq)]
pub struct AssistantText {
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub revision: u64,
    pub text: String,
    pub original_bytes: usize,
    pub truncated: bool,
}

impl fmt::Debug for AssistantText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssistantText")
            .field("turn_id", &self.turn_id)
            .field("revision", &self.revision)
            .field("bytes", &self.text.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct WireFrame(pub(super) Vec<u8>);

impl WireFrame {
    /// One bounded JSON frame including its LF terminator. Write as bytes to
    /// the owned stdin pipe; never interpolate it into a command string.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for WireFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WireFrame({} bytes)", self.0.len())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionEffect {
    Write(WireFrame),
    /// Fixture input whose actual write/flush receipt must be acknowledged.
    WriteTurn {
        request_id: Uuid,
        turn_id: Uuid,
        frame: WireFrame,
    },
    CloseInput,
}

/// Private fixture evidence. It is deliberately absent from serializable DTOs.
#[derive(Clone, Eq, PartialEq)]
pub struct ApprovalChallenge {
    pub challenge_id: Uuid,
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub vendor_session_id: String,
    pub vendor_request_id: String,
    pub tool_use_id: String,
    pub tool: ToolName,
    pub vendor_tool_name: String,
    pub input: serde_json::Value,
}

impl fmt::Debug for ApprovalChallenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApprovalChallenge")
            .field("challenge_id", &self.challenge_id)
            .field("request_id", &self.request_id)
            .field("turn_id", &self.turn_id)
            .field("tool", &self.tool)
            .finish_non_exhaustive()
    }
}

/// Private tool content. Publication requires an explicit daemon content sink.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolObservation {
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub record: ToolRecord,
}

impl fmt::Debug for ToolObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolObservation")
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

/// Private replacement observation; a command start grants no execution authority.
#[derive(Clone, Eq, PartialEq)]
pub struct CommandProgress {
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub call_id: String,
    pub revision: u64,
    pub command: String,
    pub command_original_bytes: u64,
    pub command_truncated: bool,
    pub working_directory: String,
    pub directory_original_bytes: u64,
    pub directory_truncated: bool,
    pub output_tail: Option<String>,
    pub output_original_bytes: Option<u64>,
    pub output_omitted_bytes: u64,
}

impl fmt::Debug for CommandProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandProgress")
            .field("request_id", &self.request_id)
            .field("turn_id", &self.turn_id)
            .field("revision", &self.revision)
            .field("command_original_bytes", &self.command_original_bytes)
            .field("command_truncated", &self.command_truncated)
            .field("directory_original_bytes", &self.directory_original_bytes)
            .field("directory_truncated", &self.directory_truncated)
            .field("output_original_bytes", &self.output_original_bytes)
            .field("output_omitted_bytes", &self.output_omitted_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionTransition {
    pub effects: Vec<SessionEffect>,
    pub events: Vec<SessionEvent>,
    pub text: Vec<AssistantText>,
    pub approvals: Vec<ApprovalChallenge>,
    pub tools: Vec<ToolObservation>,
    pub command_progress: Vec<CommandProgress>,
    /// Sticky optional progress loss, independent of terminal Tool-v1 capture.
    pub command_progress_limited: bool,
    /// Sticky display loss for this turn; it does not alter vendor outcome.
    pub tool_capture_incomplete: bool,
}

impl SessionTransition {
    pub(super) fn extend(&mut self, other: Self) {
        self.effects.extend(other.effects);
        self.events.extend(other.events);
        self.approvals.extend(other.approvals);
        for tool in other.tools {
            self.command_progress.retain(|progress| {
                progress.request_id != tool.request_id
                    || progress.turn_id != tool.turn_id
                    || progress.call_id != tool.record.call_id()
            });
            self.tools.push(tool);
        }
        self.command_progress_limited |= other.command_progress_limited;
        self.tool_capture_incomplete |= other.tool_capture_incomplete;
        for update in other.command_progress {
            if self.tools.iter().any(|tool| {
                tool.request_id == update.request_id
                    && tool.turn_id == update.turn_id
                    && tool.record.call_id() == update.call_id
            }) {
                continue;
            }
            if let Some(previous) = self.command_progress.iter_mut().find(|previous| {
                previous.request_id == update.request_id
                    && previous.turn_id == update.turn_id
                    && previous.call_id == update.call_id
            }) {
                if update.revision > previous.revision {
                    *previous = update;
                }
            } else {
                self.command_progress.push(update);
            }
        }
        for update in other.text {
            if let Some(previous) = self
                .text
                .iter_mut()
                .find(|previous| previous.turn_id == update.turn_id)
            {
                *previous = update;
            } else {
                self.text.push(update);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum SessionError {
    #[error("harness_chat_invalid_configuration")]
    InvalidConfiguration,
    #[error("harness_chat_invalid_identifier")]
    InvalidIdentifier,
    #[error("harness_chat_invalid_prompt")]
    InvalidPrompt,
    #[error("harness_chat_unexpected_state")]
    UnexpectedState,
    #[error("harness_chat_duplicate_request")]
    DuplicateRequest,
    #[error("harness_chat_turn_budget_exhausted")]
    TurnBudgetExhausted,
    #[error("harness_chat_rpc_budget_exhausted")]
    RpcBudgetExhausted,
    #[error("harness_chat_record_budget_exhausted")]
    RecordBudgetExhausted,
    #[error("harness_chat_frame_too_large")]
    FrameTooLarge,
    #[error("harness_chat_invalid_json")]
    InvalidJson,
    #[error("harness_chat_invalid_frame")]
    InvalidFrame,
    #[error("harness_chat_sequence_replay")]
    SequenceReplay,
    #[error("harness_chat_sequence_gap")]
    SequenceGap,
    #[error("harness_chat_uncorrelated_response")]
    UncorrelatedResponse,
    #[error("harness_chat_uncorrelated_session")]
    UncorrelatedSession,
    #[error("harness_chat_uncorrelated_turn")]
    UncorrelatedTurn,
    #[error("harness_chat_conflicting_record")]
    ConflictingRecord,
    #[error("harness_chat_unsafe_profile")]
    UnsafeProfile,
    #[error("harness_chat_vendor_request_failed")]
    VendorRequestFailed,
    #[error("harness_chat_missing_correlation")]
    MissingCorrelation,
    #[error("harness_chat_unsolicited_resume")]
    UnsolicitedResume,
    #[error("harness_chat_invalid_terminal")]
    InvalidTerminal,
    #[error("harness_chat_pending_overflow")]
    PendingOverflow,
    #[error("harness_chat_text_item_limit")]
    TextItemLimit,
    #[error("harness_chat_transport_lost")]
    TransportLost,
}

pub(super) fn validate_identifier(value: &str) -> Result<(), SessionError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        Err(SessionError::InvalidIdentifier)
    } else {
        Ok(())
    }
}
