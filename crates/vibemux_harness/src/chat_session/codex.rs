//! Codex 0.166.0 stable app-server shapes; no experimental/writable methods.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::dispatch::Sha256Digest;

use super::{
    DeniedCapability, IgnoredReason, SessionError, SessionEvent, SessionMachine, SessionPhase,
    SessionTarget, SessionTransition, TurnOutcome,
    command_progress::CommandProgressState,
    session::{RpcKind, identifier, rpc_id},
    tool_records::ToolState,
};

#[derive(Default)]
pub(super) struct CodexState {
    early: Vec<Value>,
    early_bytes: usize,
    started: bool,
    pub(super) items: BTreeMap<String, Option<Sha256Digest>>,
    pub(super) tools: ToolState,
    pub(super) command_progress: CommandProgressState,
}

impl CodexState {
    pub(super) fn reset_turn(&mut self) {
        *self = Self::default();
    }
}

impl SessionMachine {
    pub(super) fn receive_codex(
        &mut self,
        value: Value,
        bytes: usize,
    ) -> Result<SessionTransition, SessionError> {
        if value.get("jsonrpc").is_some_and(|version| version != "2.0") {
            return Err(SessionError::InvalidFrame);
        }
        if let Some(method) = value.get("method") {
            let method = identifier(Some(method))?;
            if value.get("result").is_some() || value.get("error").is_some() {
                return Err(SessionError::InvalidFrame);
            }
            if value.get("id").is_some() {
                return self.deny_codex_request(&method, &value);
            }
            if is_turn_notification(&method)
                && self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.summary.vendor_turn_id.is_none())
            {
                if self.codex.early.len() >= self.limits.max_pending_frames
                    || self.codex.early_bytes.saturating_add(bytes) > self.limits.max_pending_bytes
                {
                    return Err(SessionError::PendingOverflow);
                }
                self.codex.early_bytes += bytes;
                self.codex.early.push(value);
                return Ok(SessionTransition::default());
            }
            return self.codex_notification(&method, &value);
        }
        let id = identifier(value.get("id"))?;
        let Some(kind) = self.take_rpc(&id)? else {
            return Ok(Self::ignored(IgnoredReason::Duplicate));
        };
        let has_result = value.get("result").is_some();
        let has_error = value.get("error").is_some();
        if has_result == has_error {
            return Err(SessionError::InvalidFrame);
        }
        if let RpcKind::Interrupt(turn_id) = kind {
            if self
                .active
                .as_ref()
                .is_none_or(|active| active.summary.turn_id != turn_id)
            {
                return Ok(Self::ignored(IgnoredReason::RetiredTurn));
            }
            // An interrupt response, successful or failed, is not terminal.
            return Ok(SessionTransition {
                events: vec![if has_error {
                    SessionEvent::CancellationUnconfirmed { turn_id }
                } else {
                    SessionEvent::CancellationAcknowledged { turn_id }
                }],
                ..Default::default()
            });
        }
        if has_error {
            return Err(SessionError::VendorRequestFailed);
        }
        let result = value
            .get("result")
            .filter(|result| result.is_object())
            .ok_or(SessionError::InvalidFrame)?;
        match kind {
            RpcKind::Initialize => self.codex_initialized(result),
            RpcKind::Open => self.codex_opened(result),
            RpcKind::StartTurn(turn_id) => {
                let vendor_turn = identifier(result.pointer("/turn/id"))?;
                if self.retired_vendor_turns.contains(&vendor_turn) {
                    return Err(SessionError::UncorrelatedTurn);
                }
                let active = self.active.as_mut().ok_or(SessionError::UncorrelatedTurn)?;
                if active.summary.turn_id != turn_id || active.summary.vendor_turn_id.is_some() {
                    return Err(SessionError::UncorrelatedTurn);
                }
                if !matches!(
                    result.pointer("/turn/status").and_then(Value::as_str),
                    Some("inProgress" | "completed" | "failed" | "interrupted")
                ) {
                    return Err(SessionError::InvalidFrame);
                }
                active.summary.vendor_turn_id = Some(vendor_turn);
                let cancel = active.summary.cancel_requested;
                let mut transition = self.acknowledge()?;
                if cancel && let Some(effect) = self.cancel_effect()? {
                    transition.effects.push(effect);
                }
                let early = std::mem::take(&mut self.codex.early);
                self.codex.early_bytes = 0;
                for notification in early {
                    let method = identifier(notification.get("method"))?;
                    transition.extend(self.codex_notification(&method, &notification)?);
                }
                Ok(transition)
            }
            RpcKind::Interrupt(_) => Err(SessionError::UnexpectedState),
        }
    }

    fn codex_initialized(&mut self, result: &Value) -> Result<SessionTransition, SessionError> {
        if self.phase != SessionPhase::Initializing {
            return Err(SessionError::UnexpectedState);
        }
        identifier(result.get("userAgent"))?;
        let mut params = json!({"cwd":self.config.working_directory,"approvalPolicy":"never","sandbox":"read-only"});
        if let Some(model) = &self.config.model {
            params["model"] = json!(model);
        }
        let method = if let SessionTarget::Codex {
            resume_thread_id: Some(id),
        } = &self.config.target
        {
            params["threadId"] = json!(id);
            params["excludeTurns"] = json!(true);
            "thread/resume"
        } else {
            params["ephemeral"] = json!(false);
            "thread/start"
        };
        let initialized = self.frame(json!({"method":"initialized"}))?;
        let open = self.rpc(RpcKind::Open, method, params)?;
        self.phase = SessionPhase::Opening;
        Ok(SessionTransition {
            effects: vec![initialized, open],
            events: vec![SessionEvent::Initialized],
            text: Vec::new(),
            approvals: Vec::new(),
            ..Default::default()
        })
    }

    fn codex_opened(&mut self, result: &Value) -> Result<SessionTransition, SessionError> {
        if self.phase != SessionPhase::Opening {
            return Err(SessionError::UnexpectedState);
        }
        if result.get("approvalPolicy").and_then(Value::as_str) != Some("never")
            || result.pointer("/sandbox/type").and_then(Value::as_str) != Some("readOnly")
            || result
                .pointer("/sandbox/networkAccess")
                .is_some_and(|enabled| enabled != false)
            || result.get("cwd").and_then(Value::as_str)
                != Some(self.config.working_directory.as_str())
            || result.pointer("/thread/ephemeral") != Some(&Value::Bool(false))
            || result
                .pointer("/thread/status/type")
                .and_then(Value::as_str)
                != Some("idle")
        {
            return Err(SessionError::UnsafeProfile);
        }
        let thread = identifier(result.pointer("/thread/id"))?;
        if result
            .pointer("/thread/sessionId")
            .is_some_and(|root| root.as_str() != Some(thread.as_str()))
        {
            return Err(SessionError::UncorrelatedSession);
        }
        if self.config.model.as_ref().is_some_and(|expected| {
            result.get("model").and_then(Value::as_str) != Some(expected.as_str())
        }) {
            return Err(SessionError::UnsafeProfile);
        }
        let transition = self.bind_session(&thread)?;
        self.phase = SessionPhase::Ready;
        Ok(transition)
    }

    fn codex_notification(
        &mut self,
        method: &str,
        value: &Value,
    ) -> Result<SessionTransition, SessionError> {
        if !is_turn_notification(method)
            && !matches!(
                method,
                "thread/started" | "thread/closed" | "thread/status/changed"
            )
        {
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        let params = value
            .get("params")
            .filter(|params| params.is_object())
            .ok_or(SessionError::InvalidFrame)?;
        if method == "thread/started" {
            if let Some(expected) = &self.vendor_session_id
                && params.pointer("/thread/id").and_then(Value::as_str) != Some(expected.as_str())
            {
                return Err(SessionError::UncorrelatedSession);
            }
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        if method == "thread/status/changed"
            && self.phase == SessionPhase::Opening
            && self.vendor_session_id.is_none()
        {
            let thread_id = identifier(params.get("threadId"))?;
            if self
                .config
                .target
                .expected_id()
                .is_some_and(|expected| expected != thread_id)
            {
                return Err(SessionError::UncorrelatedSession);
            }
            // Codex can publish status before the resume response. Only that
            // response may qualify the profile, bind identity and grant Ready.
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        if params.get("threadId").is_some() {
            self.codex_thread(params)?;
        }
        if method == "thread/closed" {
            return Ok(self.transport_lost());
        }
        if !is_turn_notification(method) {
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        self.codex_thread(params)?;
        let vendor_turn = if matches!(method, "turn/started" | "turn/completed") {
            identifier(params.pointer("/turn/id"))?
        } else {
            identifier(params.get("turnId"))?
        };
        if self.retired_vendor_turns.contains(&vendor_turn) {
            return Ok(Self::ignored(IgnoredReason::RetiredTurn));
        }
        let active = self.active.as_ref().ok_or(SessionError::UncorrelatedTurn)?;
        if active.summary.vendor_turn_id.as_deref() != Some(vendor_turn.as_str()) {
            return Err(SessionError::UncorrelatedTurn);
        }
        let mut transition = match method {
            "turn/started" => {
                if params.pointer("/turn/status").and_then(Value::as_str) != Some("inProgress") {
                    return Err(SessionError::InvalidFrame);
                }
                if self.codex.started {
                    return Ok(Self::ignored(IgnoredReason::Duplicate));
                }
                self.codex.started = true;
                Ok(SessionTransition::default())
            }
            "turn/completed" => {
                let outcome = match params.pointer("/turn/status").and_then(Value::as_str) {
                    Some("completed") => TurnOutcome::Completed,
                    Some("failed") => TurnOutcome::Failed,
                    Some("interrupted") => TurnOutcome::Cancelled,
                    _ => return Err(SessionError::InvalidTerminal),
                };
                let mut transition = SessionTransition::default();
                if let Some(items) = params.pointer("/turn/items").and_then(Value::as_array) {
                    // Tool display has its own finite loss-reporting budget;
                    // it cannot consume the existing non-tool item budget
                    // and convert a valid terminal into a protocol failure.
                    if items
                        .iter()
                        .filter(|item| {
                            !matches!(
                                item.get("type").and_then(Value::as_str),
                                Some("commandExecution" | "fileChange")
                            )
                        })
                        .take(self.limits.max_text_items.saturating_add(1))
                        .count()
                        > self.limits.max_text_items
                    {
                        return Err(SessionError::TextItemLimit);
                    }
                    for item in items {
                        match item.get("type").and_then(Value::as_str) {
                            Some("agentMessage") => transition.extend(self.codex_item(item, true)?),
                            Some("commandExecution" | "fileChange") => {
                                transition.extend(self.codex_tool_item(item)?)
                            }
                            _ => {}
                        }
                    }
                } else if params.pointer("/turn/items").is_some() {
                    self.codex.tools.incomplete = true;
                }
                transition.tool_capture_incomplete |= self.codex.tools.incomplete;
                transition.extend(self.finish_turn(outcome)?);
                Ok(transition)
            }
            "item/agentMessage/delta" => {
                let id = identifier(params.get("itemId"))?;
                let delta = params
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or(SessionError::InvalidFrame)?;
                if self.codex.items.get(&id).is_some_and(Option::is_some) {
                    return Err(SessionError::ConflictingRecord);
                }
                self.remember_codex_item(&id)?;
                Ok(SessionTransition {
                    text: vec![self.text_update(&id, delta, true)?],
                    tool_capture_incomplete: self.codex.tools.incomplete,
                    ..Default::default()
                })
            }
            "item/commandExecution/outputDelta" => Ok(self.codex_command_delta(params)),
            "item/started" | "item/completed" => {
                let Some(item) = params.get("item").filter(|item| item.is_object()) else {
                    if method == "item/started" {
                        return Ok(self.malformed_command_progress());
                    }
                    return Err(SessionError::InvalidFrame);
                };
                match item.get("type").and_then(Value::as_str) {
                    Some("agentMessage") => self.codex_item(item, method == "item/completed"),
                    Some("commandExecution") if method == "item/started" => {
                        Ok(self.codex_command_started(item))
                    }
                    Some("commandExecution" | "fileChange") if method == "item/completed" => {
                        self.codex_tool_item(item)
                    }
                    _ => Ok(Self::ignored(IgnoredReason::UnknownNotification)),
                }
            }
            _ => Ok(Self::ignored(IgnoredReason::UnknownNotification)),
        }?;
        transition.command_progress_limited |= self.codex.command_progress.limited;
        Ok(transition)
    }

    fn codex_thread(&self, params: &Value) -> Result<(), SessionError> {
        let id = identifier(params.get("threadId"))?;
        if self.vendor_session_id.as_deref() != Some(id.as_str()) {
            Err(SessionError::UncorrelatedSession)
        } else {
            Ok(())
        }
    }

    fn remember_codex_item(&mut self, id: &str) -> Result<(), SessionError> {
        self.codex.command_progress.text_collision(id);
        if self.codex.tools.contains_id(id) {
            self.codex.tools.incomplete = true;
        }
        if !self.codex.items.contains_key(id) {
            if self.codex.items.len() >= self.limits.max_text_items {
                return Err(SessionError::TextItemLimit);
            }
            self.codex.items.insert(id.to_owned(), None);
        }
        Ok(())
    }

    fn codex_item(
        &mut self,
        item: &Value,
        completed: bool,
    ) -> Result<SessionTransition, SessionError> {
        let id = identifier(item.get("id"))?;
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .ok_or(SessionError::InvalidFrame)?;
        let digest = Sha256Digest::of(text.as_bytes());
        if let Some(Some(previous)) = self.codex.items.get(&id) {
            return if *previous == digest {
                Ok(Self::ignored(IgnoredReason::Duplicate))
            } else {
                Err(SessionError::ConflictingRecord)
            };
        }
        if !completed && self.codex.items.contains_key(&id) {
            return Ok(Self::ignored(IgnoredReason::Duplicate));
        }
        self.remember_codex_item(&id)?;
        if completed {
            self.codex.items.insert(id.clone(), Some(digest));
        }
        Ok(SessionTransition {
            text: vec![self.text_update(&id, text, false)?],
            tool_capture_incomplete: self.codex.tools.incomplete,
            ..Default::default()
        })
    }

    fn deny_codex_request(
        &mut self,
        method: &str,
        value: &Value,
    ) -> Result<SessionTransition, SessionError> {
        rpc_id(value.get("id"))?;
        let id = value.get("id").ok_or(SessionError::InvalidIdentifier)?;
        let key = id.to_string();
        let digest = Sha256Digest::of(value.to_string().as_bytes());
        if let Some(previous) = self.server_requests.get(&key) {
            return if *previous == digest {
                Ok(Self::ignored(IgnoredReason::Duplicate))
            } else {
                Err(SessionError::ConflictingRecord)
            };
        }
        if self.server_requests.len() >= self.limits.max_rpc_requests {
            return Err(SessionError::RpcBudgetExhausted);
        }
        self.server_requests.insert(key, digest);
        let capability = match method {
            "item/commandExecution/requestApproval" => DeniedCapability::Command,
            "item/fileChange/requestApproval" => DeniedCapability::FileChange,
            "item/permissions/requestApproval" => DeniedCapability::Permissions,
            "item/tool/requestUserInput" => DeniedCapability::UserInput,
            "item/tool/call" => DeniedCapability::Tool,
            "mcpServer/elicitation/request" => DeniedCapability::Mcp,
            _ => DeniedCapability::Unknown,
        };
        // A denial grants nothing, including for an unsolicited/foreign ID.
        // No tool arguments, paths, or permission objects are reflected back.
        let response = if matches!(
            capability,
            DeniedCapability::Command | DeniedCapability::FileChange
        ) {
            json!({"id":id,"result":{"decision":"decline"}})
        } else {
            json!({"id":id,"error":{"code":-32601,"message":"Client capability not granted"}})
        };
        Ok(SessionTransition {
            effects: vec![self.frame(response)?],
            events: vec![SessionEvent::CapabilityDenied { capability }],
            text: Vec::new(),
            approvals: Vec::new(),
            ..Default::default()
        })
    }
}

fn is_turn_notification(method: &str) -> bool {
    matches!(
        method,
        "turn/started"
            | "turn/completed"
            | "item/started"
            | "item/completed"
            | "item/agentMessage/delta"
            | "item/commandExecution/outputDelta"
    )
}
