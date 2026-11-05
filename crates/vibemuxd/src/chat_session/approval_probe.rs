//! Pure field-level diagnostics for a contained, deny-only Claude probe.
//! This observer supplies no approval authority or native-profile qualification.

use serde::Serialize;
use serde_json::Value;
use std::{fmt, io};
use uuid::Uuid;

const MAX_ID_BYTES: usize = 512;
const MAX_RECORDS: u16 = 128;
const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_PENDING_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeError {
    InvalidConfiguration,
    RecordLimit,
    RecordBytesLimit,
    InputLimit,
    PendingLimit,
    InvalidIdentifier,
    MalformedRecord,
    ForeignSession,
    NestedIdentity,
    ForeignInput,
    InitMismatch,
    ReplayIdentity,
    ConflictingIdentity,
    UnboundStream,
    UnexpectedTool,
    ConflictingTool,
    AdditionalChallenge,
    ToolMismatch,
    MissingRequest,
    AlreadyDenied,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(output, "approval_probe_{self:?}")
    }
}

impl std::error::Error for ProbeError {}

/// Exact candidate comparisons after root identity validation; never raw values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProbeInitChecks {
    pub permission_mode_matches: bool,
    pub working_directory_matches: bool,
    pub tools_match: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProbeReport {
    pub root_init_seen: bool,
    /// None means no identity-qualified initialization reached these checks.
    pub init_checks: Option<ProbeInitChecks>,
    pub current_input_echo_seen: bool,
    pub missing_input_echo_seen: bool,
    pub complete_tool_seen: bool,
    pub matching_request_seen: bool,
    pub matched_tool_current_input: bool,
    pub proof_before_reply: bool,
    pub denied: bool,
    pub terminal_seen: bool,
    pub record_count: u16,
    /// Encoded JSON bytes, excluding transport framing and whitespace.
    pub record_bytes: u64,
    pub pending_bytes: u64,
    pub error: Option<ProbeError>,
}

struct MessageBinding {
    id: String,
    retired: bool,
    correlated: bool,
    snapshot_seen: bool,
}

struct ToolEvidence {
    id: String,
    input: Value,
    message: usize,
}

struct PendingRequest {
    id: String,
    tool_id: String,
    input: Value,
}

struct StreamTool {
    index: u64,
    id: String,
    input_json: String,
    initial_nonempty: bool,
    has_delta: bool,
}

struct StreamMessage {
    binding: usize,
    tool: Option<StreamTool>,
}

pub(crate) struct ProbeObserver {
    session_id: Uuid,
    user_id: Uuid,
    cwd: String,
    record_ids: Vec<String>,
    messages: Vec<MessageBinding>,
    stream: Option<StreamMessage>,
    tool: Option<ToolEvidence>,
    request: Option<PendingRequest>,
    report: ProbeReport,
}

impl fmt::Debug for ProbeObserver {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_struct("ProbeObserver")
            .field("report", &self.report())
            .finish_non_exhaustive()
    }
}

impl ProbeObserver {
    pub fn new(session_id: Uuid, user_id: Uuid, cwd: String) -> Result<Self, ProbeError> {
        if session_id.is_nil()
            || user_id.is_nil()
            || cwd.is_empty()
            || cwd.len() > MAX_PENDING_BYTES
        {
            return Err(ProbeError::InvalidConfiguration);
        }
        let report = ProbeReport {
            pending_bytes: cwd.len() as u64,
            ..ProbeReport::default()
        };
        Ok(Self {
            session_id,
            user_id,
            cwd,
            record_ids: Vec::new(),
            messages: Vec::new(),
            stream: None,
            tool: None,
            request: None,
            report,
        })
    }

    pub fn observe(&mut self, frame: &Value) -> Result<(), ProbeError> {
        if let Some(error) = self.report.error {
            return Err(error);
        }
        let result = self.observe_record(frame);
        if let Err(error) = result {
            self.report.error = Some(error);
        }
        result
    }

    // The sole private identity accessor is for the parent's fixed DENY reply.
    pub fn pending_request_id(&self) -> Option<&str> {
        if self.report.denied {
            None
        } else {
            self.request.as_ref().map(|request| request.id.as_str())
        }
    }

    pub fn mark_denied(&mut self) -> Result<(), ProbeError> {
        if self.report.denied {
            let error = *self.report.error.get_or_insert(ProbeError::AlreadyDenied);
            return Err(error);
        }
        if self.request.is_none() {
            let error = *self.report.error.get_or_insert(ProbeError::MissingRequest);
            return Err(error);
        }
        self.report.proof_before_reply = self.report.error.is_none()
            && self.report.root_init_seen
            && self.report.matched_tool_current_input
            && self.report.complete_tool_seen
            && self.report.matching_request_seen
            && !self.report.terminal_seen;
        self.report.denied = true;
        self.report.error.map_or(Ok(()), Err)
    }

    pub fn report(&self) -> ProbeReport {
        self.report
    }

    fn observe_record(&mut self, frame: &Value) -> Result<(), ProbeError> {
        if self.report.record_count == MAX_RECORDS {
            return Err(ProbeError::RecordLimit);
        }
        let remaining = MAX_RECORD_BYTES - self.report.record_bytes as usize;
        let bytes = encoded_len(frame, remaining, ProbeError::RecordBytesLimit)?;
        self.report.record_count += 1;
        self.report.record_bytes += bytes as u64;
        if self.report.terminal_seen
            && matches!(
                frame["type"].as_str(),
                Some("assistant" | "stream_event" | "control_request")
            )
        {
            return Err(ProbeError::ReplayIdentity);
        }
        if let Some(id) = frame.get("uuid") {
            let id = identifier(id)?;
            if self.record_ids.iter().any(|record_id| record_id == id) {
                return Err(ProbeError::ReplayIdentity);
            }
            self.hold(id.len())?;
            self.record_ids.push(id.to_owned());
        }
        match frame["type"].as_str() {
            Some("system") if frame["subtype"] == "init" => self.observe_init(frame)?,
            Some("assistant") => self.observe_assistant(frame)?,
            Some("stream_event") => self.observe_stream(frame)?,
            Some("control_request") if frame["request"]["subtype"] == "can_use_tool" => {
                self.observe_request(frame)?;
            }
            Some("result") => {
                self.check_root(frame, true, false)?;
                self.report.terminal_seen = true;
            }
            // A replayed root user UUID is data, not an assistant correlation echo.
            _ => {}
        }
        if let (Some(tool), Some(request)) = (&self.tool, &self.request) {
            if tool.id != request.tool_id || tool.input != request.input {
                return Err(ProbeError::ToolMismatch);
            }
            self.report.matching_request_seen = true;
            self.report.matched_tool_current_input = self.messages[tool.message].correlated;
        }
        Ok(())
    }

    fn observe_init(&mut self, frame: &Value) -> Result<(), ProbeError> {
        self.check_root(frame, true, false)?;
        if self.report.root_init_seen {
            return Err(ProbeError::ReplayIdentity);
        }
        let checks = ProbeInitChecks {
            // SDK events use canonical `default`; existing fixtures emit the
            // CLI `manual` alias. Accept only these two exact spellings.
            permission_mode_matches: matches!(
                frame["permissionMode"].as_str(),
                Some("manual" | "default")
            ),
            working_directory_matches: frame["cwd"].as_str() == Some(self.cwd.as_str()),
            tools_match: frame["tools"]
                .as_array()
                .is_some_and(|tools| tools.len() == 1 && tools[0] == "Write"),
        };
        self.report.init_checks = Some(checks);
        if !checks.permission_mode_matches
            || !checks.working_directory_matches
            || !checks.tools_match
        {
            return Err(ProbeError::InitMismatch);
        }
        self.report.root_init_seen = true;
        Ok(())
    }

    fn observe_assistant(&mut self, frame: &Value) -> Result<(), ProbeError> {
        self.check_root(frame, true, true)?;
        if self.stream.is_some() {
            return Err(ProbeError::ConflictingIdentity);
        }
        let message = &frame["message"];
        self.check_root(message, false, false)?;
        if message["role"] != "assistant" {
            return Err(ProbeError::MalformedRecord);
        }
        let message_id = identifier(&message["id"])?;
        let binding = if let Some(index) = self.messages.iter().position(|v| v.id == message_id) {
            if self.messages[index].snapshot_seen {
                return Err(ProbeError::ReplayIdentity);
            }
            index
        } else {
            self.bind_message(frame, &message["id"])?
        };
        self.check_message_reference(frame, &self.messages[binding].id)?;
        let content = message["content"]
            .as_array()
            .ok_or(ProbeError::MalformedRecord)?;
        let mut tool_block_seen = false;
        for block in content {
            if block["type"] == "tool_use" {
                if tool_block_seen {
                    return Err(ProbeError::ConflictingTool);
                }
                tool_block_seen = true;
                self.check_root(block, false, false)?;
                self.check_tool_name(&block["name"])?;
                let id = identifier(&block["id"])?;
                if let Some(tool) = &self.tool {
                    if tool.message != binding || tool.id != id || tool.input != block["input"] {
                        return Err(ProbeError::ConflictingTool);
                    }
                    continue;
                }
                let bytes = input_len(&block["input"])?;
                self.hold(id.len() + bytes)?;
                self.tool = Some(ToolEvidence {
                    id: id.to_owned(),
                    input: block["input"].clone(),
                    message: binding,
                });
                self.report.complete_tool_seen = true;
            }
        }
        // A final snapshot must preserve its streamed tool before its echo
        // can establish this message's current-input provenance.
        if self
            .tool
            .as_ref()
            .is_some_and(|tool| tool.message == binding)
            && !tool_block_seen
        {
            return Err(ProbeError::ConflictingTool);
        }
        if frame.get("user_message_uuid").is_some() {
            self.messages[binding].correlated = true;
            self.report.current_input_echo_seen = true;
        }
        self.messages[binding].retired = true;
        self.messages[binding].snapshot_seen = true;
        Ok(())
    }

    fn observe_request(&mut self, frame: &Value) -> Result<(), ProbeError> {
        if self.request.is_some() {
            return Err(ProbeError::AdditionalChallenge);
        }
        self.check_root(frame, false, false)?;
        let request = &frame["request"];
        self.check_root(request, false, false)?;
        self.check_tool_name(&request["tool_name"])?;
        let id = identifier(&frame["request_id"])?;
        let tool_id = identifier(&request["tool_use_id"])?;
        let bytes = input_len(&request["input"])?;
        self.hold(id.len() + tool_id.len() + bytes)?;
        self.request = Some(PendingRequest {
            id: id.to_owned(),
            tool_id: tool_id.to_owned(),
            input: request["input"].clone(),
        });
        Ok(())
    }

    fn observe_stream(&mut self, frame: &Value) -> Result<(), ProbeError> {
        self.check_root(frame, true, true)?;
        let event = &frame["event"];
        self.check_root(event, false, false)?;
        if event["type"] == "message_start" {
            if self.stream.is_some() {
                return Err(ProbeError::ConflictingIdentity);
            }
            let message = &event["message"];
            self.check_root(message, false, false)?;
            if message["role"] != "assistant" {
                return Err(ProbeError::MalformedRecord);
            }
            let binding = self.bind_message(frame, &message["id"])?;
            self.check_message_reference(frame, &self.messages[binding].id)?;
            self.check_message_reference(event, &self.messages[binding].id)?;
            self.stream = Some(StreamMessage {
                binding,
                tool: None,
            });
            return Ok(());
        }
        let mut stream = self.stream.take().ok_or(ProbeError::UnboundStream)?;
        self.check_message_reference(frame, &self.messages[stream.binding].id)?;
        self.check_message_reference(event, &self.messages[stream.binding].id)?;
        match event["type"].as_str() {
            Some("content_block_start") => {
                let block = &event["content_block"];
                self.check_root(block, false, false)?;
                if block["type"] == "tool_use" {
                    if stream.tool.is_some() || self.tool.is_some() {
                        return Err(ProbeError::ConflictingTool);
                    }
                    self.check_tool_name(&block["name"])?;
                    let id = identifier(&block["id"])?;
                    let bytes = input_len(&block["input"])?;
                    let index = event["index"].as_u64().ok_or(ProbeError::MalformedRecord)?;
                    self.hold(id.len() + bytes)?;
                    let input_json = serde_json::to_string(&block["input"])
                        .map_err(|_| ProbeError::MalformedRecord)?;
                    stream.tool = Some(StreamTool {
                        index,
                        id: id.to_owned(),
                        input_json,
                        initial_nonempty: block["input"].as_object().is_some_and(|v| !v.is_empty()),
                        has_delta: false,
                    });
                } else if stream.tool.is_some() {
                    return Err(ProbeError::ConflictingTool);
                }
            }
            Some("content_block_delta") => {
                if event["delta"]["type"] == "input_json_delta" {
                    let tool = stream.tool.as_mut().ok_or(ProbeError::UnboundStream)?;
                    if event["index"].as_u64() != Some(tool.index) || tool.initial_nonempty {
                        return Err(ProbeError::ConflictingTool);
                    }
                    let partial = event["delta"]["partial_json"]
                        .as_str()
                        .ok_or(ProbeError::MalformedRecord)?;
                    let retained = if tool.has_delta {
                        tool.input_json.len()
                    } else {
                        0
                    };
                    if partial.len() > MAX_INPUT_BYTES.saturating_sub(retained) {
                        return Err(ProbeError::InputLimit);
                    }
                    self.hold(partial.len())?;
                    if !tool.has_delta {
                        self.report.pending_bytes -= tool.input_json.len() as u64;
                        tool.input_json.clear();
                        tool.has_delta = true;
                    }
                    tool.input_json.push_str(partial);
                } else if stream.tool.is_some() {
                    return Err(ProbeError::MalformedRecord);
                }
            }
            Some("content_block_stop") => {
                if let Some(tool) = stream.tool.take() {
                    if event["index"].as_u64() != Some(tool.index) {
                        return Err(ProbeError::ConflictingTool);
                    }
                    let input: Value = serde_json::from_str(&tool.input_json)
                        .map_err(|_| ProbeError::MalformedRecord)?;
                    // Account the parsed encoding, which can differ from the
                    // bounded source spelling of numbers and escapes.
                    self.hold(input_len(&input)?)?;
                    self.report.pending_bytes -= tool.input_json.len() as u64;
                    self.tool = Some(ToolEvidence {
                        id: tool.id,
                        input,
                        message: stream.binding,
                    });
                    self.report.complete_tool_seen = true;
                }
            }
            Some("message_stop") => {
                if stream.tool.is_some() {
                    return Err(ProbeError::MalformedRecord);
                }
                self.messages[stream.binding].retired = true;
                return Ok(());
            }
            Some("message_delta") => {}
            _ => return Err(ProbeError::MalformedRecord),
        }
        self.stream = Some(stream);
        Ok(())
    }

    fn bind_message(&mut self, frame: &Value, id: &Value) -> Result<usize, ProbeError> {
        let id = identifier(id)?;
        if let Some(message) = self.messages.iter().find(|message| message.id == id) {
            return Err(if message.retired {
                ProbeError::ReplayIdentity
            } else {
                ProbeError::ConflictingIdentity
            });
        }
        let correlated = frame.get("user_message_uuid").is_some();
        if let Some(echo) = frame.get("user_message_uuid")
            && uuid_field(echo)? != self.user_id
        {
            return Err(ProbeError::ForeignInput);
        }
        if let Some(echoes) = frame.get("user_message_uuids") {
            let echoes = echoes.as_array().ok_or(ProbeError::ForeignInput)?;
            if echoes.len() != 1 || uuid_field(&echoes[0])? != self.user_id {
                return Err(ProbeError::ForeignInput);
            }
        }
        self.hold(id.len())?;
        let binding = self.messages.len();
        self.messages.push(MessageBinding {
            id: id.to_owned(),
            retired: false,
            correlated,
            snapshot_seen: false,
        });
        self.report.current_input_echo_seen |= correlated;
        self.report.missing_input_echo_seen |= !correlated;
        Ok(binding)
    }

    fn check_root(
        &self,
        value: &Value,
        require_session: bool,
        require_parent: bool,
    ) -> Result<(), ProbeError> {
        match value.get("session_id") {
            Some(session) if uuid_field(session)? != self.session_id => {
                return Err(ProbeError::ForeignSession);
            }
            None if require_session => return Err(ProbeError::ForeignSession),
            _ => {}
        }
        if require_parent && value.get("parent_tool_use_id") != Some(&Value::Null) {
            return Err(ProbeError::NestedIdentity);
        }
        for name in [
            "parent_tool_use_id",
            "agent_id",
            "subagent_id",
            "parent_agent_id",
        ] {
            if value.get(name).is_some_and(|field| !field.is_null()) {
                return Err(ProbeError::NestedIdentity);
            }
        }
        if value.get("isReplay") == Some(&Value::Bool(true))
            || value.get("resume_reason").is_some_and(|v| !v.is_null())
            || value.get("origin").is_some_and(|v| {
                !v.is_null() && v.get("kind").and_then(Value::as_str) != Some("human")
            })
        {
            return Err(ProbeError::ReplayIdentity);
        }
        if let Some(echo) = value.get("user_message_uuid")
            && uuid_field(echo)? != self.user_id
        {
            return Err(ProbeError::ForeignInput);
        }
        if let Some(echoes) = value.get("user_message_uuids") {
            let echoes = echoes.as_array().ok_or(ProbeError::ForeignInput)?;
            if echoes.len() != 1 || uuid_field(&echoes[0])? != self.user_id {
                return Err(ProbeError::ForeignInput);
            }
        }
        Ok(())
    }

    fn check_message_reference(&self, value: &Value, id: &str) -> Result<(), ProbeError> {
        if let Some(message_id) = value.get("message_id")
            && identifier(message_id)? != id
        {
            return Err(ProbeError::ConflictingIdentity);
        }
        Ok(())
    }

    fn check_tool_name(&self, value: &Value) -> Result<(), ProbeError> {
        if identifier(value)? != "Write" {
            return Err(ProbeError::UnexpectedTool);
        }
        Ok(())
    }

    fn hold(&mut self, bytes: usize) -> Result<(), ProbeError> {
        // Account encoded payload/identifier bytes; allocator metadata is not measured.
        if bytes > MAX_PENDING_BYTES.saturating_sub(self.report.pending_bytes as usize) {
            return Err(ProbeError::PendingLimit);
        }
        self.report.pending_bytes += bytes as u64;
        Ok(())
    }
}

fn identifier(value: &Value) -> Result<&str, ProbeError> {
    let id = value.as_str().ok_or(ProbeError::InvalidIdentifier)?;
    if id.is_empty() || id.len() > MAX_ID_BYTES || id.chars().any(char::is_control) {
        return Err(ProbeError::InvalidIdentifier);
    }
    Ok(id)
}

fn uuid_field(value: &Value) -> Result<Uuid, ProbeError> {
    Uuid::parse_str(identifier(value)?).map_err(|_| ProbeError::InvalidIdentifier)
}

fn input_len(value: &Value) -> Result<usize, ProbeError> {
    if !value.is_object() {
        return Err(ProbeError::MalformedRecord);
    }
    encoded_len(value, MAX_INPUT_BYTES, ProbeError::InputLimit)
}

fn encoded_len(value: &Value, limit: usize, error: ProbeError) -> Result<usize, ProbeError> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(io::Error::other("probe_byte_limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| error)?;
    Ok(counter.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn observer() -> ProbeObserver {
        ProbeObserver::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            "C:/private_probe".into(),
        )
        .expect("observer")
    }

    fn init() -> Value {
        json!({"type":"system","subtype":"init","session_id":Uuid::from_u128(1),
            "cwd":"C:/private_probe","permissionMode":"manual","tools":["Write"]})
    }

    fn input() -> Value {
        json!({"file_path":"C:/private_candidate","content":"private content"})
    }

    fn assistant() -> Value {
        json!({"type":"assistant","session_id":Uuid::from_u128(1),
            "parent_tool_use_id":null,"user_message_uuid":Uuid::from_u128(2),
            "message":{"id":"private_message","role":"assistant","content":[{
                "type":"tool_use","id":"private_tool","name":"Write","input":input()}]}})
    }

    fn request() -> Value {
        json!({"type":"control_request","request_id":"private_request","request":{
            "subtype":"can_use_tool","tool_name":"Write","tool_use_id":"private_tool",
            "input":input()}})
    }

    fn stream(event: Value) -> Value {
        json!({"type":"stream_event","session_id":Uuid::from_u128(1),
            "parent_tool_use_id":null,"event":event})
    }

    fn stream_start() -> Value {
        let mut frame = stream(json!({"type":"message_start","message":{
            "id":"private_stream","role":"assistant","content":[]}}));
        frame["user_message_uuid"] = json!(Uuid::from_u128(2));
        frame
    }

    #[test]
    fn canonical_manual_mode_reaches_only_correlated_deny_evidence() {
        for mode in ["manual", "default"] {
            let mut observer = observer();
            let mut initialization = init();
            initialization["permissionMode"] = json!(mode);
            observer
                .observe(&initialization)
                .expect("documented manual-mode spelling");
            assert_eq!(
                observer.report().init_checks,
                Some(ProbeInitChecks {
                    permission_mode_matches: true,
                    working_directory_matches: true,
                    tools_match: true,
                })
            );
            assert!(!observer.report().proof_before_reply);
            assert!(observer.pending_request_id().is_none());
            observer
                .observe(&assistant())
                .expect("exact input-bound tool");
            observer
                .observe(&request())
                .expect("matching permission request");
            observer.mark_denied().expect("fixed deny only");
            assert!(observer.report().proof_before_reply && observer.report().denied);
            assert!(observer.pending_request_id().is_none());
        }
    }

    #[test]
    fn other_permission_modes_and_malformed_aliases_remain_rejected() {
        for mode in [
            json!("acceptEdits"),
            json!("auto"),
            json!("bypassPermissions"),
            json!("dontAsk"),
            json!("plan"),
            json!("Default"),
            json!("manual "),
            json!(""),
            Value::Null,
            json!(true),
            json!(0),
            json!(["default"]),
            json!({"mode":"default"}),
        ] {
            let mut observer = observer();
            let mut initialization = init();
            initialization["permissionMode"] = mode;
            assert_eq!(
                observer.observe(&initialization),
                Err(ProbeError::InitMismatch)
            );
            assert_eq!(
                observer.report().init_checks,
                Some(ProbeInitChecks {
                    permission_mode_matches: false,
                    working_directory_matches: true,
                    tools_match: true,
                })
            );
            assert!(!observer.report().root_init_seen && !observer.report().proof_before_reply);
            assert!(observer.pending_request_id().is_none());
            assert_eq!(
                observer.observe(&assistant()),
                Err(ProbeError::InitMismatch)
            );
        }
    }

    #[test]
    fn nil_origins_and_historical_or_nested_root_metadata_cannot_prove_origin() {
        assert!(ProbeObserver::new(Uuid::nil(), Uuid::from_u128(2), "C:/probe".into()).is_err());
        assert!(ProbeObserver::new(Uuid::from_u128(1), Uuid::nil(), "C:/probe".into()).is_err());
        for (key, value) in [
            ("parent_agent_id", json!("nested")),
            ("isReplay", json!(true)),
            ("resume_reason", json!("recovered")),
            ("origin", json!({"kind":"agent"})),
        ] {
            let mut observer = observer();
            let mut frame = assistant();
            frame[key] = value;
            assert!(observer.observe(&frame).is_err(), "{key}");
            assert!(!observer.report().proof_before_reply);
        }
    }

    #[test]
    fn a_complete_snapshot_can_confirm_its_own_echo_bound_stream_once() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        observer.observe(&stream_start()).expect("bound stream");
        observer.observe(&stream(json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"private_tool","name":"Write","input":input()}}))).expect("tool start");
        observer
            .observe(&stream(json!({"type":"content_block_stop","index":0})))
            .expect("complete tool");
        observer
            .observe(&stream(json!({"type":"message_stop"})))
            .expect("stream stop");
        let mut snapshot = assistant();
        snapshot["message"]["id"] = json!("private_stream");
        snapshot
            .as_object_mut()
            .expect("object")
            .remove("user_message_uuid");
        observer.observe(&snapshot).expect("same final snapshot");
        observer.observe(&request()).expect("request");
        observer.mark_denied().expect("deny");
        assert!(observer.report().proof_before_reply);
        assert!(
            observer.observe(&snapshot).is_err(),
            "second complete snapshot is a replay"
        );
    }

    #[test]
    fn an_echoed_snapshot_cannot_attach_an_omitted_streamed_tool_to_the_input() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        let mut start = stream_start();
        start
            .as_object_mut()
            .expect("object")
            .remove("user_message_uuid");
        observer.observe(&start).expect("raw stream");
        observer.observe(&stream(json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"private_tool","name":"Write","input":input()}}))).expect("tool start");
        observer
            .observe(&stream(json!({"type":"content_block_stop","index":0})))
            .expect("tool complete");
        observer
            .observe(&stream(json!({"type":"message_stop"})))
            .expect("stop");
        observer.observe(&request()).expect("matching request");
        let mut snapshot = assistant();
        snapshot["message"]["id"] = json!("private_stream");
        snapshot["message"]["content"] = json!([]);
        assert_eq!(
            observer.observe(&snapshot),
            Err(ProbeError::ConflictingTool)
        );
        let _ = observer.mark_denied();
        assert!(!observer.report().proof_before_reply);
        assert!(!observer.report().matched_tool_current_input);
    }

    #[test]
    fn root_user_replay_cannot_supply_the_missing_assistant_echo() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        observer
            .observe(&json!({"type":"user","uuid":Uuid::from_u128(2),
            "session_id":Uuid::from_u128(1)}))
            .expect("user data");
        let mut frame = assistant();
        frame
            .as_object_mut()
            .expect("object")
            .remove("user_message_uuid");
        observer.observe(&frame).expect("uncorrelated observation");
        assert!(!observer.report().current_input_echo_seen);
        assert!(observer.report().missing_input_echo_seen);
        assert!(observer.report().complete_tool_seen);
        observer.observe(&request()).expect("raw matching request");
        observer
            .mark_denied()
            .expect("deny without current-input proof");
        assert!(!observer.report().proof_before_reply);
    }

    #[test]
    fn an_unrelated_echo_cannot_bind_an_earlier_uncorrelated_tool() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        let mut unbound = assistant();
        unbound
            .as_object_mut()
            .expect("object")
            .remove("user_message_uuid");
        observer.observe(&unbound).expect("raw tool observation");
        observer.observe(&request()).expect("raw request match");
        let mut unrelated = assistant();
        unrelated["message"]["id"] = json!("unrelated_message");
        unrelated["message"]["content"] = json!([]);
        observer
            .observe(&unrelated)
            .expect("another echoed message");
        assert!(observer.report().current_input_echo_seen);
        assert!(observer.report().matching_request_seen);
        assert!(!observer.report().matched_tool_current_input);
        observer.mark_denied().expect("deny");
        assert!(!observer.report().proof_before_reply);
    }

    #[test]
    fn complete_echo_bound_tool_matches_and_denial_retires_the_request() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        let mut frame = assistant();
        frame["user_message_uuids"] = json!([Uuid::from_u128(2)]);
        observer.observe(&frame).expect("tool");
        observer.observe(&request()).expect("request");
        assert_eq!(observer.pending_request_id(), Some("private_request"));
        observer.mark_denied().expect("denied");
        assert!(observer.report().proof_before_reply);
        assert!(observer.report().denied);
        assert_eq!(observer.pending_request_id(), None);
    }

    #[test]
    fn a_stream_requires_an_echo_bound_start_and_complete_json() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        observer.observe(&stream_start()).expect("bound start");
        observer
            .observe(&stream(json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"private_tool","name":"Write","input":{}}})))
            .expect("tool start");
        let encoded = serde_json::to_string(&input()).expect("input JSON");
        for partial in [&encoded[..10], &encoded[10..]] {
            observer
                .observe(&stream(json!({"type":"content_block_delta","index":0,
                "delta":{"type":"input_json_delta","partial_json":partial}})))
                .expect("delta");
        }
        observer.observe(&request()).expect("early request");
        assert!(!observer.report().complete_tool_seen);
        assert!(!observer.report().matching_request_seen);
        observer
            .observe(&stream(json!({"type":"content_block_stop","index":0})))
            .expect("complete");
        observer
            .observe(&stream(json!({"type":"message_stop"})))
            .expect("retire stream");
        observer.mark_denied().expect("denied");
        assert!(observer.report().proof_before_reply);
    }

    #[test]
    fn an_early_request_can_match_before_denial_but_late_proof_cannot_rewrite_it() {
        for deny_early in [false, true] {
            let mut observer = observer();
            observer.observe(&init()).expect("init");
            observer.observe(&request()).expect("buffered request");
            if deny_early {
                observer.mark_denied().expect("early denial");
            }
            observer.observe(&assistant()).expect("later tool evidence");
            assert!(observer.report().matching_request_seen);
            if !deny_early {
                observer.mark_denied().expect("denial with proof");
            }
            assert_eq!(observer.report().proof_before_reply, !deny_early);
        }
    }

    #[test]
    fn foreign_nested_and_ambiguous_echoes_are_fixed_failures() {
        let mut cases = Vec::new();
        let mut foreign = assistant();
        foreign["session_id"] = json!(Uuid::from_u128(3));
        cases.push((foreign, ProbeError::ForeignSession));
        let mut nested = assistant();
        nested["parent_tool_use_id"] = json!("private_parent");
        cases.push((nested, ProbeError::NestedIdentity));
        let mut multiple = assistant();
        multiple["user_message_uuids"] = json!([Uuid::from_u128(2), Uuid::from_u128(3)]);
        cases.push((multiple, ProbeError::ForeignInput));
        for (frame, error) in cases {
            let mut observer = observer();
            assert_eq!(observer.observe(&frame), Err(error));
            assert_eq!(observer.observe(&assistant()), Err(error));
            assert!(!observer.report().proof_before_reply);
        }
    }

    #[test]
    fn changed_input_duplicate_challenges_and_retired_messages_are_rejected() {
        let mut repeated_block = observer();
        let mut repeated = assistant();
        let block = repeated["message"]["content"][0].clone();
        repeated["message"]["content"]
            .as_array_mut()
            .expect("content")
            .push(block);
        assert_eq!(
            repeated_block.observe(&repeated),
            Err(ProbeError::ConflictingTool)
        );
        let mut changed = observer();
        changed.observe(&assistant()).expect("tool");
        let mut frame = request();
        frame["request"]["input"]["content"] = json!("changed private content");
        assert_eq!(changed.observe(&frame), Err(ProbeError::ToolMismatch));
        let mut duplicate = observer();
        duplicate.observe(&request()).expect("first challenge");
        assert_eq!(
            duplicate.observe(&request()),
            Err(ProbeError::AdditionalChallenge)
        );
        let mut retired = observer();
        retired.observe(&assistant()).expect("first message");
        assert_eq!(
            retired.observe(&assistant()),
            Err(ProbeError::ReplayIdentity)
        );
    }

    #[test]
    fn stream_continuations_without_a_bound_start_are_rejected() {
        let mut unbound = observer();
        assert_eq!(
            unbound.observe(&stream(json!({"type":"content_block_stop","index":0}))),
            Err(ProbeError::UnboundStream)
        );
        let mut observer = observer();
        let mut start = stream_start();
        start
            .as_object_mut()
            .expect("object")
            .remove("user_message_uuid");
        observer.observe(&start).expect("raw stream observation");
        assert!(!observer.report().current_input_echo_seen);
        assert!(observer.report().missing_input_echo_seen);
    }

    #[test]
    fn limits_are_applied_before_retaining_inputs_ids_or_stream_chunks() {
        let mut oversized = observer();
        let mut frame = request();
        frame["request"]["input"]["content"] = json!("x".repeat(MAX_INPUT_BYTES));
        assert_eq!(oversized.observe(&frame), Err(ProbeError::InputLimit));
        assert_eq!(oversized.pending_request_id(), None);
        let mut long_id = observer();
        let mut frame = request();
        frame["request_id"] = json!("x".repeat(MAX_ID_BYTES + 1));
        assert_eq!(long_id.observe(&frame), Err(ProbeError::InvalidIdentifier));
        let mut records = observer();
        for _ in 0..MAX_RECORDS {
            records
                .observe(&json!({"type":"ignored"}))
                .expect("bounded record");
        }
        assert_eq!(records.observe(&json!({})), Err(ProbeError::RecordLimit));
        let mut bytes = observer();
        assert_eq!(
            bytes.observe(&json!({"ignored":"x".repeat(MAX_RECORD_BYTES)})),
            Err(ProbeError::RecordBytesLimit)
        );
        assert_eq!(bytes.report().record_bytes, 0);
        let mut pending = ProbeObserver::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            "x".repeat(MAX_PENDING_BYTES),
        )
        .expect("bounded cwd");
        assert_eq!(pending.observe(&assistant()), Err(ProbeError::PendingLimit));
        assert!(pending.report().pending_bytes <= MAX_PENDING_BYTES as u64);
        let mut streamed = observer();
        streamed.observe(&stream_start()).expect("start");
        streamed
            .observe(&stream(json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"private_tool","name":"Write","input":{}}})))
            .expect("tool start");
        assert_eq!(
            streamed.observe(&stream(json!({"type":"content_block_delta","index":0,
            "delta":{"type":"input_json_delta","partial_json":"x".repeat(MAX_INPUT_BYTES + 1)}}))),
            Err(ProbeError::InputLimit)
        );
        assert!(!streamed.report().complete_tool_seen);
    }

    #[test]
    fn replayed_record_identity_cannot_supply_more_evidence() {
        let mut observer = observer();
        let mut frame = stream_start();
        frame["uuid"] = json!("private_record");
        observer.observe(&frame).expect("first record");
        let mut replay = stream(json!({"type":"message_delta"}));
        replay["uuid"] = json!("private_record");
        assert_eq!(observer.observe(&replay), Err(ProbeError::ReplayIdentity));
    }

    #[test]
    fn conflicting_stream_identity_and_terminal_replays_cannot_supply_proof() {
        let mut conflicting = observer();
        conflicting.observe(&stream_start()).expect("bound start");
        let mut delta = stream(json!({"type":"message_delta","message_id":"foreign_message"}));
        assert_eq!(
            conflicting.observe(&delta),
            Err(ProbeError::ConflictingIdentity)
        );
        let mut foreign = observer();
        foreign.observe(&stream_start()).expect("bound start");
        delta["event"]
            .as_object_mut()
            .expect("event")
            .remove("message_id");
        delta["user_message_uuid"] = json!(Uuid::from_u128(3));
        assert_eq!(foreign.observe(&delta), Err(ProbeError::ForeignInput));
        let mut terminal = observer();
        terminal.observe(&request()).expect("request");
        terminal
            .observe(&json!({"type":"result","session_id":Uuid::from_u128(1)}))
            .expect("terminal record");
        assert_eq!(
            terminal.observe(&assistant()),
            Err(ProbeError::ReplayIdentity)
        );
        assert!(!terminal.report().proof_before_reply);
    }

    #[test]
    fn reports_and_debug_never_include_private_values() {
        let mut observer = observer();
        observer.observe(&init()).expect("init");
        observer.observe(&assistant()).expect("tool");
        observer.observe(&request()).expect("request");
        let report = serde_json::to_string(&observer.report()).expect("safe report");
        let debug = format!("{observer:?}");
        for secret in [
            "private_probe",
            "private_candidate",
            "private content",
            "private_message",
            "private_tool",
            "private_request",
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
        ] {
            assert!(!report.contains(secret));
            assert!(!debug.contains(secret));
        }
    }
}
