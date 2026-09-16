#![forbid(unsafe_code)]
//! Harness orchestration surface for the Rust core: the ten-harness profile
//! registry, machine-readable row construction, snapshot/switch payloads, and
//! canonical event drafts. This crate is pure logic — it performs no process,
//! filesystem, or network I/O; detection inputs are injected by callers (the
//! daemon reads the trusted `vibemux_probe` cache), which keeps the types ->
//! harness -> store dependency direction acyclic per AGENTS.md §4.2.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use vibemux_probe::{AgentKind, LauncherKind, ProbeState};

pub const HARNESS_REGISTRY_ENTITY_KIND: &str = "harness_registry";
pub const HARNESS_CONFIG_ENTITY_KIND: &str = "harness_config";
pub const MAX_HARNESS_PATH_BYTES: usize = 1024;
pub const MAX_HARNESS_ROLES: usize = 64;
pub const MAX_HARNESS_ROLE_BYTES: usize = 128;
pub const MAX_HARNESS_LAUNCHER_BYTES: usize = 32;
pub const MAX_HARNESS_VERSION_BYTES: usize = 256;
/// The Python prototype never carries a persisted default (its sidecar stores
/// only detection state); the Rust store seeds this sentinel instead of a
/// fake harness name, and `switch` refuses to emit it as a real selection.
pub const NO_DEFAULT_HARNESS: &str = "none";
pub const HARNESS_PROBED_EVENT: &str = "harness_probed";
pub const HARNESS_SWITCHED_EVENT: &str = "harness_switched";

/// Launch-transport classification retained from the Python
/// `HarnessProtocol` behavior reference. Rust launch/attach execution is not
/// implemented in this slice; the label is persisted for parity and future
/// adapters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessProtocol {
    Pty,
}

/// Static profile for one harness: identity plus the launch surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessProfile {
    pub agent: AgentKind,
    pub command: &'static str,
    pub protocol: HarnessProtocol,
    pub provider: &'static str,
}

/// Per-harness detection input consumed by row building. `detected` must be
/// derived from a launcher that the probe actually verified (`--version`
/// succeeded); `path`/`launcher`/`version` are retained for observability and
/// never interpolated into shell strings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HarnessDetection {
    pub detected: bool,
    pub path: Option<String>,
    pub launcher: Option<LauncherKind>,
    pub version: Option<String>,
}

/// One row of the machine-readable harness listing, mirroring the Python
/// `vibemux harnesses --json` row contract (plus Rust observability fields).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessRow {
    pub name: String,
    pub command: Vec<String>,
    pub protocol: HarnessProtocol,
    pub provider: String,
    pub available: bool,
    pub path: Option<String>,
    pub roles: Vec<String>,
    pub default: bool,
    #[serde(default)]
    pub launcher: Option<LauncherKind>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Persisted per-harness snapshot state (registry projection value).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessState {
    pub detected: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

impl HarnessState {
    fn validate(&self) -> Result<(), HarnessError> {
        if self.roles.len() > MAX_HARNESS_ROLES {
            return Err(HarnessError::InvalidState);
        }
        if self
            .roles
            .iter()
            .any(|role| role.is_empty() || role.len() > MAX_HARNESS_ROLE_BYTES)
        {
            return Err(HarnessError::InvalidState);
        }
        if let Some(path) = &self.path {
            validate_path(path)?;
        }
        Ok(())
    }
}

/// Persisted registry projection: one entry per known harness.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessRegistrySnapshot {
    #[serde(default)]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub harnesses: Vec<HarnessEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessEntry {
    pub name: String,
    #[serde(flatten)]
    pub state: HarnessState,
}

impl HarnessRegistrySnapshot {
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.harnesses.len() != AgentKind::all().len() {
            return Err(HarnessError::InvalidState);
        }
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.harnesses {
            if profile_by_name(&entry.name).is_none() || !seen.insert(entry.name.as_str()) {
                return Err(HarnessError::InvalidState);
            }
            entry.state.validate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn state(&self, name: &str) -> Option<&HarnessState> {
        self.harnesses
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| &entry.state)
    }
}

/// Persisted default-harness projection value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfigSnapshot {
    pub default_harness: String,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HarnessError {
    #[error("unknown harness name")]
    UnknownHarness,
    #[error("harness snapshot state is invalid")]
    InvalidState,
    #[error("harness path exceeds the bounded length or is not portable UTF-8")]
    InvalidPath,
}

impl HarnessError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownHarness => "harness_unknown",
            Self::InvalidState => "harness_invalid_state",
            Self::InvalidPath => "harness_invalid_path",
        }
    }
}

/// The ten supported harnesses, in canonical probe/dashboard order.
#[must_use]
pub fn profiles() -> [HarnessProfile; 10] {
    AgentKind::all().map(|agent| HarnessProfile {
        agent,
        command: agent.command_name(),
        protocol: HarnessProtocol::Pty,
        provider: match agent {
            AgentKind::Claude => "anthropic",
            AgentKind::Codex | AgentKind::Copilot => "openai",
            AgentKind::OpenCode => "sst",
            AgentKind::Grok => "xai",
            AgentKind::Qwen => "alibaba",
            AgentKind::Iflow => "iflow",
            AgentKind::Trae => "bytedance",
            AgentKind::Codebuddy => "tencent",
            AgentKind::Kimi => "moonshot",
        },
    })
}

#[must_use]
pub fn profile_for(agent: AgentKind) -> HarnessProfile {
    profiles()[agent as usize]
}

#[must_use]
pub fn profile_by_name(name: &str) -> Option<HarnessProfile> {
    profiles()
        .into_iter()
        .find(|profile| profile.command == name)
}

fn validate_path(path: &str) -> Result<(), HarnessError> {
    // The path is observability-only (never executed, never shelled out), so
    // portability validation is a bounded-length check plus rejection of NUL,
    // which cannot survive JSON round trips into process APIs safely anyway.
    if path.is_empty() || path.len() > MAX_HARNESS_PATH_BYTES || path.contains('\0') {
        return Err(HarnessError::InvalidPath);
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), HarnessError> {
    if version.is_empty() || version.len() > MAX_HARNESS_VERSION_BYTES || version.contains('\0') {
        return Err(HarnessError::InvalidState);
    }
    Ok(())
}

/// Build the listing rows by zipping the static profiles with the previous
/// snapshot's roles and fresh (or cached) detection results.
pub fn build_rows(
    registry: &HarnessRegistrySnapshot,
    config: &HarnessConfigSnapshot,
    detections: &std::collections::BTreeMap<String, HarnessDetection>,
) -> Vec<HarnessRow> {
    profiles()
        .into_iter()
        .map(|profile| {
            let state = registry.state(profile.command);
            let detection = detections.get(profile.command);
            let detected = detection.is_some_and(|input| input.detected);
            HarnessRow {
                name: profile.command.to_string(),
                command: vec![profile.command.to_string()],
                protocol: profile.protocol,
                provider: profile.provider.to_string(),
                available: detected,
                path: if detected {
                    detection.and_then(|input| input.path.clone())
                } else {
                    None
                },
                roles: state.map(|state| state.roles.clone()).unwrap_or_default(),
                default: profile.command == config.default_harness,
                launcher: detection.and_then(|input| input.launcher),
                version: detection.and_then(|input| input.version.clone()),
            }
        })
        .collect()
}

/// Payload + persisted registry state produced by a refresh: roles carry over
/// from the previous snapshot, detection replaces availability.
pub fn build_refresh(
    previous: &HarnessRegistrySnapshot,
    detections: &std::collections::BTreeMap<String, HarnessDetection>,
    checked_at: &str,
) -> (HarnessRegistrySnapshot, Value) {
    let mut harnesses = Vec::with_capacity(AgentKind::all().len());
    let mut detected = Vec::new();
    let mut missing = Vec::new();
    for profile in profiles() {
        let input = detections.get(profile.command);
        let is_detected = input.is_some_and(|input| input.detected);
        if is_detected {
            detected.push(profile.command);
        } else {
            missing.push(profile.command);
        }
        harnesses.push(HarnessEntry {
            name: profile.command.to_string(),
            state: HarnessState {
                detected: is_detected,
                path: if is_detected {
                    input.and_then(|input| input.path.clone())
                } else {
                    None
                },
                roles: previous
                    .state(profile.command)
                    .map(|state| state.roles.clone())
                    .unwrap_or_default(),
            },
        });
    }
    let snapshot = HarnessRegistrySnapshot {
        checked_at: Some(checked_at.to_string()),
        harnesses,
    };
    let payload = json!({
        "detected": detected,
        "missing": missing,
    });
    (snapshot, payload)
}

/// Event payload for a refresh without building the snapshot (the store
/// carries roles over internally). Mirrors `build_refresh`'s payload.
#[must_use]
pub fn refresh_payload(detections: &std::collections::BTreeMap<String, HarnessDetection>) -> Value {
    let mut detected = Vec::new();
    let mut missing = Vec::new();
    for profile in profiles() {
        if detections
            .get(profile.command)
            .is_some_and(|input| input.detected)
        {
            detected.push(profile.command);
        } else {
            missing.push(profile.command);
        }
    }
    json!({
        "detected": detected,
        "missing": missing,
    })
}

/// Seed registry written by the store migration: nothing detected, no roles,
/// no timestamp — explicit so the projection exists before the first refresh.
#[must_use]
pub fn seed_registry() -> HarnessRegistrySnapshot {
    HarnessRegistrySnapshot {
        checked_at: None,
        harnesses: profiles()
            .into_iter()
            .map(|profile| HarnessEntry {
                name: profile.command.to_string(),
                state: HarnessState {
                    detected: false,
                    path: None,
                    roles: Vec::new(),
                },
            })
            .collect(),
    }
}

#[must_use]
pub fn seed_config() -> HarnessConfigSnapshot {
    HarnessConfigSnapshot {
        default_harness: NO_DEFAULT_HARNESS.to_string(),
    }
}

/// Event drafts for the writer. The draft is built here so the event-type and
/// payload shape stay canonical across the store migration and the daemon.
/// Timestamps are minted at commit time (drafts are consumed immediately, not
/// replayed from disk), so `OffsetDateTime::now_utc()` is correct here.
pub mod events {
    use super::*;
    use time::OffsetDateTime;
    use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
    use vibemux_types::{EventId, ProjectId};

    fn draft(
        event_type: &str,
        project_id: ProjectId,
        idempotency_key: String,
        payload: Value,
    ) -> Result<EventDraft, HarnessError> {
        let draft = EventDraft {
            event_id: EventId::new(),
            event_type: EventType::new(event_type).map_err(|_| HarnessError::InvalidState)?,
            project_id,
            task_id: None,
            run_id: None,
            causation_id: None,
            actor: ActorName::new("vibemuxd").map_err(|_| HarnessError::InvalidState)?,
            timestamp: OffsetDateTime::now_utc(),
            idempotency_key: Some(idempotency_key),
            payload: EventPayload::new(payload).map_err(|_| HarnessError::InvalidState)?,
        };
        draft.validate().map_err(|_| HarnessError::InvalidState)?;
        Ok(draft)
    }

    pub fn probed(
        project_id: ProjectId,
        idempotency_key: String,
        payload: Value,
    ) -> Result<EventDraft, HarnessError> {
        draft(HARNESS_PROBED_EVENT, project_id, idempotency_key, payload)
    }

    pub fn switched(
        project_id: ProjectId,
        idempotency_key: String,
        from: &str,
        to: &str,
    ) -> Result<EventDraft, HarnessError> {
        draft(
            HARNESS_SWITCHED_EVENT,
            project_id,
            idempotency_key,
            json!({"from": from, "to": to}),
        )
    }

    pub fn seed(project_id: ProjectId) -> Result<EventDraft, HarnessError> {
        draft(
            "v3_harness_seed",
            project_id,
            "v3-harness-seed".to_string(),
            json!({"migration": 3}),
        )
    }
}

/// Detection input derived from a trusted probe-cache agent entry. Callers
/// pass `launcher_state` through unchanged; detection requires the verified
/// state so a mere PATH hit never counts as available.
pub fn detection_from_probe(
    launcher_state: ProbeState,
    launcher: LauncherKind,
    version: Option<&str>,
    path: Option<&str>,
) -> Result<HarnessDetection, HarnessError> {
    let detected = launcher_state == ProbeState::Verified;
    let path = match (detected, path) {
        (true, Some(path)) => {
            validate_path(path)?;
            Some(path.to_string())
        }
        _ => None,
    };
    let version = match (detected, version) {
        (true, Some(version)) => {
            validate_version(version)?;
            Some(version.to_string())
        }
        _ => None,
    };
    Ok(HarnessDetection {
        detected,
        path,
        launcher: detected.then_some(launcher),
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use vibemux_types::ProjectId;

    fn detection(detected: bool) -> HarnessDetection {
        HarnessDetection {
            detected,
            path: detected.then(|| "C:\\tools\\claude.exe".to_string()),
            launcher: detected.then_some(LauncherKind::DirectExecutable),
            version: detected.then(|| "1.0.0".to_string()),
        }
    }

    #[test]
    fn profiles_cover_all_ten_agents_in_probe_order() {
        let profiles = profiles();
        assert_eq!(profiles.len(), 10);
        for (profile, agent) in profiles.iter().zip(AgentKind::all()) {
            assert_eq!(profile.agent, agent);
            assert_eq!(profile.command, agent.command_name());
            assert_eq!(profile.protocol, HarnessProtocol::Pty);
            assert!(!profile.provider.is_empty());
        }
    }

    #[test]
    fn rows_mark_default_and_preserve_roles() {
        let mut registry = seed_registry();
        registry
            .harnesses
            .iter_mut()
            .find(|entry| entry.name == "claude")
            .expect("claude entry")
            .state
            .roles = vec!["worker".to_string(), "reviewer".to_string()];
        let config = HarnessConfigSnapshot {
            default_harness: "claude".to_string(),
        };
        let detections = BTreeMap::from([("claude".to_string(), detection(true))]);
        let rows = build_rows(&registry, &config, &detections);
        assert_eq!(rows.len(), 10);
        let claude = rows.iter().find(|row| row.name == "claude").expect("row");
        assert!(claude.available);
        assert!(claude.default);
        assert_eq!(claude.roles, ["worker", "reviewer"]);
        assert_eq!(claude.command, ["claude"]);
        assert_eq!(claude.path.as_deref(), Some("C:\\tools\\claude.exe"));
        assert_eq!(claude.version.as_deref(), Some("1.0.0"));
        let codex = rows.iter().find(|row| row.name == "codex").expect("row");
        assert!(!codex.available);
        assert!(!codex.default);
        assert_eq!(codex.path, None);
    }

    #[test]
    fn refresh_carries_roles_and_builds_sorted_payload() {
        let mut previous = seed_registry();
        previous
            .harnesses
            .iter_mut()
            .find(|entry| entry.name == "kimi")
            .expect("kimi entry")
            .state
            .roles = vec!["orchestrator".to_string()];
        let detections = BTreeMap::from([
            ("claude".to_string(), detection(true)),
            ("qwen".to_string(), detection(true)),
            ("codex".to_string(), detection(false)),
        ]);
        let (snapshot, payload) = build_refresh(&previous, &detections, "2026-09-16T00:00:00Z");
        assert_eq!(snapshot.checked_at.as_deref(), Some("2026-09-16T00:00:00Z"));
        // Profile order is alphabetical (claude, codex, ..., trae), matching
        // the sorted payload the Python reference emits.
        assert_eq!(
            payload,
            json!({"detected": ["claude", "qwen"], "missing": ["codex", "opencode", "copilot", "grok", "iflow", "trae", "codebuddy", "kimi"]})
        );
        let kimi = snapshot.state("kimi").expect("kimi state");
        assert!(!kimi.detected);
        assert_eq!(kimi.roles, ["orchestrator"]);
        assert!(snapshot.state("codex").expect("codex").path.is_none());
    }

    #[test]
    fn detection_requires_verified_launcher() {
        let failed = detection_from_probe(
            ProbeState::Failed,
            LauncherKind::DirectExecutable,
            Some("1.0.0"),
            Some("C:\\claude.exe"),
        )
        .expect("failed probe detection");
        assert!(!failed.detected);
        assert_eq!(failed.path, None);
        assert_eq!(failed.launcher, None);
        let verified = detection_from_probe(
            ProbeState::Verified,
            LauncherKind::PowerShellCompanion,
            Some("2.0.0"),
            Some("C:\\codex.cmd"),
        )
        .expect("verified probe detection");
        assert!(verified.detected);
        assert_eq!(verified.launcher, Some(LauncherKind::PowerShellCompanion));
    }

    #[test]
    fn snapshot_validation_rejects_foreign_or_duplicate_names() {
        let mut snapshot = seed_registry();
        snapshot.harnesses[0].name = "unknown".to_string();
        assert_eq!(
            snapshot.validate(),
            Err(HarnessError::InvalidState),
            "foreign names are rejected"
        );
        let mut snapshot = seed_registry();
        snapshot.harnesses[1].name = "claude".to_string();
        assert_eq!(snapshot.validate(), Err(HarnessError::InvalidState));
        let mut snapshot = seed_registry();
        snapshot.harnesses.pop();
        assert_eq!(snapshot.validate(), Err(HarnessError::InvalidState));
    }

    #[test]
    fn oversized_paths_and_roles_are_rejected() {
        let mut state = HarnessState {
            detected: true,
            path: Some("x".repeat(MAX_HARNESS_PATH_BYTES + 1)),
            roles: Vec::new(),
        };
        assert_eq!(state.validate(), Err(HarnessError::InvalidPath));
        state.path = None;
        state.roles = vec!["worker".to_string(); MAX_HARNESS_ROLES + 1];
        assert_eq!(state.validate(), Err(HarnessError::InvalidState));
        assert_eq!(
            detection_from_probe(
                ProbeState::Verified,
                LauncherKind::DirectExecutable,
                None,
                Some("with\0nul"),
            ),
            Err(HarnessError::InvalidPath)
        );
    }

    #[test]
    fn event_drafts_are_canonical() {
        let project_id = ProjectId::new();
        let probed = events::probed(
            project_id,
            "probe-1".to_string(),
            json!({"detected": ["claude"], "missing": []}),
        )
        .expect("probed draft");
        assert_eq!(probed.event_type.as_str(), "harness_probed");
        assert_eq!(probed.actor.as_str(), "vibemuxd");
        let switched =
            events::switched(project_id, "switch-1".to_string(), "none", "claude").expect("draft");
        assert_eq!(switched.event_type.as_str(), "harness_switched");
        assert_eq!(
            events::seed(project_id)
                .expect("seed draft")
                .idempotency_key
                .as_deref(),
            Some("v3-harness-seed")
        );
    }

    #[test]
    fn no_default_sentinel_is_not_a_switchable_harness() {
        assert!(profile_by_name(NO_DEFAULT_HARNESS).is_none());
    }
}
