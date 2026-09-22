#![forbid(unsafe_code)]
//! Harness orchestration surface for the Rust core: the agent/launcher/probe
//! state vocabulary ([`agent`], re-exported at the crate root), the
//! ten-harness profile registry, machine-readable row construction,
//! snapshot/switch payloads, and canonical event drafts. This crate is pure
//! logic — it performs no process, filesystem, or network I/O; detection
//! inputs are injected by callers (the daemon reads the trusted probe cache
//! owned by `vibemux_probe::cache`), which keeps the types -> harness ->
//! store dependency direction acyclic per AGENTS.md §4.2.

pub mod agent;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub use agent::{AgentKind, LauncherKind, ProbeState};

pub const HARNESS_REGISTRY_ENTITY_KIND: &str = "harness_registry";
pub const HARNESS_CONFIG_ENTITY_KIND: &str = "harness_config";
pub const MAX_HARNESS_PATH_BYTES: usize = 1024;
pub const MAX_HARNESS_ROLES: usize = 64;
pub const MAX_HARNESS_ROLE_BYTES: usize = 128;
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
///
/// `launcher`/`version` are optional because the persisted registry was
/// seeded before `AgentProbe::path` was added to the probe report; the
/// store treats a legacy row as equivalent to `(None, None)` so the schema-3
/// JSON shape stays forward-compatible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessState {
    pub detected: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub launcher: Option<LauncherKind>,
    #[serde(default)]
    pub version: Option<String>,
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
        if let Some(version) = &self.version {
            validate_version(version)?;
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

/// One persisted registry entry. The on-disk shape is a flat object
/// (`{"name": "...", "detected": ..., "path": ..., "roles": [...],
/// "launcher": ..., "version": ...}`) — `HarnessEntry` and `HarnessState`
/// exist as separate Rust types only to make ownership of `name` explicit;
/// `Deserialize` is implemented manually so an unknown field on the registry
/// entry fails closed (the on-disk shape is part of the schema-3 wire
/// contract), and `Serialize` keeps the same flat layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessEntry {
    pub name: String,
    pub state: HarnessState,
}

impl Serialize for HarnessEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("HarnessEntry", 6)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("detected", &self.state.detected)?;
        s.serialize_field("path", &self.state.path)?;
        s.serialize_field("roles", &self.state.roles)?;
        s.serialize_field("launcher", &self.state.launcher)?;
        s.serialize_field("version", &self.state.version)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for HarnessEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Helper {
            name: String,
            detected: bool,
            #[serde(default)]
            path: Option<String>,
            #[serde(default)]
            roles: Vec<String>,
            #[serde(default)]
            launcher: Option<LauncherKind>,
            #[serde(default)]
            version: Option<String>,
        }
        let helper = Helper::deserialize(deserializer)?;
        Ok(HarnessEntry {
            name: helper.name,
            state: HarnessState {
                detected: helper.detected,
                path: helper.path,
                roles: helper.roles,
                launcher: helper.launcher,
                version: helper.version,
            },
        })
    }
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
    #[error("harness snapshot state is invalid")]
    InvalidState,
    #[error("harness path exceeds the bounded length or is not portable UTF-8")]
    InvalidPath,
}

impl HarnessError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidState => "harness_invalid_state",
            Self::InvalidPath => "harness_invalid_path",
        }
    }
}

/// The ten supported harnesses, in canonical probe/dashboard order.
#[must_use]
pub fn profiles() -> [HarnessProfile; 10] {
    AgentKind::all().map(|agent| HarnessProfile {
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
    detections: &BTreeMap<String, HarnessDetection>,
) -> Vec<HarnessRow> {
    profiles()
        .into_iter()
        .map(|profile| {
            let state = registry.state(profile.command);
            let detection = detections.get(profile.command);
            let detected = detection.is_some_and(HarnessDetection::is_detected);
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

/// Build the listing rows from PERSISTED registry state alone (no fresh
/// detections). Used by the cached (`--cached`) path and by the daemon after
/// the writer commits a refresh, so the live and cached views share one
/// derivation.
#[must_use]
pub fn build_rows_from_registry(
    registry: &HarnessRegistrySnapshot,
    config: &HarnessConfigSnapshot,
) -> Vec<HarnessRow> {
    profiles()
        .into_iter()
        .map(|profile| {
            let state = registry.state(profile.command);
            let detected = state.is_some_and(HarnessState::is_detected);
            HarnessRow {
                name: profile.command.to_string(),
                command: vec![profile.command.to_string()],
                protocol: profile.protocol,
                provider: profile.provider.to_string(),
                available: detected,
                path: if detected {
                    state.and_then(|state| state.path.clone())
                } else {
                    None
                },
                roles: state.map(|state| state.roles.clone()).unwrap_or_default(),
                default: profile.command == config.default_harness,
                launcher: state.and_then(|state| state.launcher),
                version: state.and_then(|state| state.version.clone()),
            }
        })
        .collect()
}

/// Classify the detected/missing harness partition alphabetically, matching
/// the Python `services.py` payload order (Python iterates over a sorted
/// `dict`). This is the only classifier that produces the partition; every
/// other entry point delegates here.
fn classify_detections(
    detections: &BTreeMap<String, HarnessDetection>,
) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut detected = Vec::new();
    let mut missing = Vec::new();
    for profile in profiles() {
        if detections
            .get(profile.command)
            .is_some_and(HarnessDetection::is_detected)
        {
            detected.push(profile.command);
        } else {
            missing.push(profile.command);
        }
    }
    detected.sort_unstable();
    missing.sort_unstable();
    (detected, missing)
}

impl HarnessDetection {
    /// `true` when the detection represents a verified harness. The probe is
    /// the only source of truth — a bare PATH hit never counts.
    #[must_use]
    pub const fn is_detected(&self) -> bool {
        self.detected
    }
}

impl HarnessState {
    /// `true` when the persisted state represents a verified harness.
    #[must_use]
    pub const fn is_detected(&self) -> bool {
        self.detected
    }
}

/// Payload + persisted registry state produced by a refresh: roles carry over
/// from the previous snapshot, detection replaces availability. Payload
/// arrays are sorted alphabetically to match Python `services.py`.
pub fn build_refresh(
    previous: &HarnessRegistrySnapshot,
    detections: &BTreeMap<String, HarnessDetection>,
    checked_at: &str,
) -> (HarnessRegistrySnapshot, Value) {
    let mut harnesses = Vec::with_capacity(AgentKind::all().len());
    for profile in profiles() {
        let input = detections.get(profile.command);
        let is_detected = input.is_some_and(HarnessDetection::is_detected);
        // Fail-closed: an undetected entry never persists path/launcher/
        // version, even if the caller supplied a `HarnessDetection` with
        // those fields populated. Every upstream pipeline (the probe-derived
        // `detection_from_probe` and the daemon's degrade path) already zeros
        // them for undetected, so this is the one place that can enforce the
        // invariant independently of the caller.
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
                launcher: if is_detected {
                    input.and_then(|input| input.launcher)
                } else {
                    None
                },
                version: if is_detected {
                    input.and_then(|input| input.version.clone())
                } else {
                    None
                },
            },
        });
    }
    let (detected, missing) = classify_detections(detections);
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
                    launcher: None,
                    version: None,
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
        // Alphabetical order matches the Python `services.py` reference:
        // both detected and missing arrays are sorted ascending.
        assert_eq!(
            payload,
            json!({"detected": ["claude", "qwen"], "missing": ["codebuddy", "codex", "copilot", "grok", "iflow", "kimi", "opencode", "trae"]})
        );
        // The persisted snapshot still classifies by detection state, not
        // by the payload — claude and qwen are detected, the rest are not.
        assert!(snapshot.state("claude").expect("claude").detected);
        assert!(snapshot.state("qwen").expect("qwen").detected);
        let kimi = snapshot.state("kimi").expect("kimi state");
        assert!(!kimi.detected);
        assert_eq!(kimi.roles, ["orchestrator"]);
        assert!(snapshot.state("codex").expect("codex").path.is_none());
        assert_eq!(
            snapshot.state("claude").expect("claude").launcher,
            Some(LauncherKind::DirectExecutable)
        );
    }

    /// Regression: an undetected `HarnessDetection` that still carries a
    /// launcher/version (a caller that did NOT zero them) must never persist
    /// those fields — `build_refresh` gates `path`/`launcher`/`version` on
    /// `is_detected`, independently of the caller. This keeps the persisted
    /// registry consistent with the probe-derived and daemon-degrade paths.
    #[test]
    fn build_refresh_strips_undetected_observability_fields() {
        let previous = seed_registry();
        // A present-but-undetected detection that a buggy caller failed to
        // zero: `detected = false` but launcher/version/path populated.
        let mut detections = BTreeMap::new();
        detections.insert(
            "claude".to_string(),
            HarnessDetection {
                detected: false,
                path: Some("C:\\tools\\claude.exe".to_string()),
                launcher: Some(LauncherKind::DirectExecutable),
                version: Some("1.0.0".to_string()),
            },
        );
        let (snapshot, _payload) = build_refresh(&previous, &detections, "2026-09-16T00:00:00Z");
        let claude = snapshot.state("claude").expect("claude state");
        assert!(!claude.detected);
        assert!(claude.path.is_none(), "undetected path must not persist");
        assert!(
            claude.launcher.is_none(),
            "undetected launcher must not persist"
        );
        assert!(
            claude.version.is_none(),
            "undetected version must not persist"
        );
    }

    #[test]
    fn build_rows_from_registry_matches_build_rows_for_persisted_state() {
        let mut previous = seed_registry();
        previous
            .harnesses
            .iter_mut()
            .find(|entry| entry.name == "claude")
            .expect("claude entry")
            .state
            .roles = vec!["worker".to_string(), "reviewer".to_string()];
        let config = HarnessConfigSnapshot {
            default_harness: "claude".to_string(),
        };
        // Live detection turns claude detected.
        let detections = BTreeMap::from([("claude".to_string(), detection(true))]);
        let (persisted, _payload) = build_refresh(&previous, &detections, "2026-09-16T00:00:00Z");
        // Live rows reflect the detection input.
        let live_rows = build_rows(&previous, &config, &detections);
        // Cached rows are derived purely from the persisted state.
        let cached_rows = build_rows_from_registry(&persisted, &config);
        assert_eq!(live_rows, cached_rows);
    }

    #[test]
    fn registry_entry_unknown_field_is_rejected() {
        let encoded = r#"{"name":"claude","detected":true,"path":null,"roles":[],"launcher":null,"version":null,"extra":true}"#;
        let error = serde_json::from_str::<HarnessEntry>(encoded)
            .expect_err("unknown field must fail closed");
        assert!(error.to_string().contains("extra"));
    }

    #[test]
    fn registry_entry_legacy_without_launcher_or_version_parses() {
        // Older schema-3 rows written before launcher/version existed must
        // still parse (the harness state defaults the optional fields).
        let encoded = r#"{"name":"claude","detected":true,"path":null,"roles":[]}"#;
        let entry: HarnessEntry = serde_json::from_str(encoded).expect("legacy entry parses");
        assert_eq!(entry.name, "claude");
        assert!(entry.state.detected);
        assert_eq!(entry.state.launcher, None);
        assert_eq!(entry.state.version, None);
    }

    #[test]
    fn registry_entry_round_trip_preserves_flat_shape() {
        let entry = HarnessEntry {
            name: "claude".to_string(),
            state: HarnessState {
                detected: true,
                path: Some("C:\\tools\\claude.exe".to_string()),
                roles: vec!["worker".to_string()],
                launcher: Some(LauncherKind::DirectExecutable),
                version: Some("1.0.0".to_string()),
            },
        };
        let encoded = serde_json::to_string(&entry).expect("serialize entry");
        // Flat shape: no nested `state` object in the JSON.
        assert!(!encoded.contains("\"state\""));
        assert!(encoded.contains("\"name\":\"claude\""));
        assert!(encoded.contains("\"detected\":true"));
        assert!(encoded.contains("\"launcher\":\"direct_executable\""));
        assert!(encoded.contains("\"version\":\"1.0.0\""));
        let decoded: HarnessEntry = serde_json::from_str(&encoded).expect("round-trip");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn harness_state_validates_version_bounds() {
        let mut state = HarnessState {
            detected: true,
            path: None,
            roles: Vec::new(),
            launcher: Some(LauncherKind::DirectExecutable),
            version: Some("1.0.0".to_string()),
        };
        assert!(state.validate().is_ok());
        state.version = Some(String::new());
        assert_eq!(state.validate(), Err(HarnessError::InvalidState));
        state.version = Some("x".repeat(MAX_HARNESS_VERSION_BYTES + 1));
        assert_eq!(state.validate(), Err(HarnessError::InvalidState));
        state.version = Some("with\0nul".to_string());
        assert_eq!(state.validate(), Err(HarnessError::InvalidState));
        state.version = None;
        assert!(state.validate().is_ok());
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
            launcher: None,
            version: None,
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

    #[test]
    fn harness_error_no_longer_contains_unknown_harness_variant() {
        // Compile-time check: the variant is gone and the `harness_unknown`
        // code arm no longer exists. A constructor that would have produced
        // `HarnessError::UnknownHarness` now fails to compile, so the
        // surviving two-variant surface is the only legal harness error
        // vocabulary for callers.
        let _: HarnessError = HarnessError::InvalidState;
        let _: HarnessError = HarnessError::InvalidPath;
        assert_eq!(HarnessError::InvalidState.code(), "harness_invalid_state");
        assert_eq!(HarnessError::InvalidPath.code(), "harness_invalid_path");
    }
}
