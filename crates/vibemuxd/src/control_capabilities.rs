//! Additive, content-free capability discovery for the local control surface.
//!
//! `capabilities` is a read-only operation introduced at the existing v10
//! revision. The descriptor and its version are deliberately unchanged: every
//! shipped v1-v10 client checks the descriptor version and would refuse a
//! newer daemon outright. An older v10 daemon answers the unknown operation
//! with an invalid frame, so consumers must fall back to the legacy reads.
//!
//! The catalog separates four facts that are often conflated:
//! - what this build supports (`supported_actions`, derived from the adapter
//!   registry);
//! - what the operator configured (`configured`, from the pinned dispatch
//!   routes);
//! - what the trusted probe cache observed (`launcher_observed`; a missing
//!   cache degrades to `false`, never to a failed discovery);
//! - what is actually enabled by the route (`enabled_actions`).
//!
//! `qualified_actions` stays empty in this revision: no live qualification
//! evidence is recorded in the daemon, and fixture coverage is not a live
//! claim. No path, token, endpoint, prompt, or configuration digest is
//! exposed.

use std::path::Path;

use serde::{Deserialize, Serialize};
use vibemux_harness::dispatch::capability::{AdapterSupport, adapter_capabilities};
use vibemux_harness::dispatch::route_config::DispatchCatalogEntry;

use super::{
    CONTROL_PROTOCOL_VERSION, ControlError, ControlOperation, ControlPayload,
    MAX_CONTROL_FRAME_BYTES, ServerState,
};

/// Whether an operation changes state or starts an owned process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityAccess {
    Read,
    Write,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityOperation {
    pub name: String,
    pub minimum_version: u32,
    pub access: CapabilityAccess,
    pub request_schema: String,
    pub response_schema: String,
    pub handler: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRuntime {
    pub harness: String,
    pub profile_id: String,
    pub protocol: String,
    pub platform: String,
    pub supported_actions: Vec<String>,
    pub configured: bool,
    pub launcher_observed: bool,
    pub qualified_actions: Vec<String>,
    pub enabled_actions: Vec<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityCatalog {
    pub schema_version: u32,
    pub control_contract_id: String,
    pub protocol_version: u32,
    pub operations: Vec<CapabilityOperation>,
    pub runtimes: Vec<CapabilityRuntime>,
}

const CATALOG_SCHEMA_VERSION: u32 = 1;
const ENVELOPE_HEADROOM_BYTES: usize = 512;
const ACTION_PROBE: &str = "probe";
const ACTION_SUBMIT: &str = "submit";

fn supported_actions(support: AdapterSupport) -> Vec<String> {
    match support {
        AdapterSupport::Execute => vec![ACTION_PROBE.to_string(), ACTION_SUBMIT.to_string()],
        AdapterSupport::ProbeOnly => vec![ACTION_PROBE.to_string()],
        AdapterSupport::TerminalOnly => Vec::new(),
    }
}

fn enabled_actions(support: AdapterSupport, route: Option<&DispatchCatalogEntry>) -> Vec<String> {
    let Some(route) = route else {
        return Vec::new();
    };
    if !route.enabled {
        return Vec::new();
    }
    // An enabled action is always also a supported action: a route that allows
    // execution cannot enable a prompt turn for a harness whose adapters are
    // probe-only or terminal-only.
    let mut actions = Vec::new();
    if route.probe_supported && support != AdapterSupport::TerminalOnly {
        actions.push(ACTION_PROBE.to_string());
    }
    if route.allow_execution && support == AdapterSupport::Execute {
        actions.push(ACTION_SUBMIT.to_string());
    }
    actions
}

fn unavailable_reason(
    support: AdapterSupport,
    route: Option<&DispatchCatalogEntry>,
    enabled: &[String],
) -> Option<String> {
    if support == AdapterSupport::TerminalOnly {
        return Some("no_structured_adapter".to_string());
    }
    match route {
        None => Some("route_not_configured".to_string()),
        Some(route) if !route.enabled => Some("route_disabled".to_string()),
        Some(_) if enabled.is_empty() => Some("no_enabled_action".to_string()),
        Some(_) => None,
    }
}

fn build_catalog(routes: Vec<DispatchCatalogEntry>, probe_cache_path: &Path) -> CapabilityCatalog {
    let detections = super::load_probe_detections(probe_cache_path).unwrap_or_default();
    let operations = ControlOperation::ALL
        .iter()
        .map(|operation| CapabilityOperation {
            name: operation.wire_name(),
            minimum_version: operation.minimum_protocol_version(),
            access: if operation.is_mutating() {
                CapabilityAccess::Write
            } else {
                CapabilityAccess::Read
            },
            request_schema: "control.request.v1".to_string(),
            response_schema: "control.response.v1".to_string(),
            handler: operation.handler_group().to_string(),
        })
        .collect();
    let platform = std::env::consts::OS.to_string();
    let runtimes = adapter_capabilities()
        .into_iter()
        .map(|capability| {
            let harness = capability.harness;
            let command_name = harness.command_name().to_string();
            let route = routes.iter().find(|route| route.harness == harness);
            let supported = supported_actions(capability.support);
            let enabled = enabled_actions(capability.support, route);
            let reason = unavailable_reason(capability.support, route, &enabled);
            let (profile_id, protocol, configured) = match route {
                Some(route) => (
                    format!("{command_name}.{}", route.protocol.as_str()),
                    route.protocol.as_str().to_string(),
                    true,
                ),
                None => (
                    format!("{command_name}.unconfigured"),
                    capability
                        .protocols
                        .first()
                        .map(|protocol| protocol.as_str())
                        .unwrap_or("none")
                        .to_string(),
                    false,
                ),
            };
            CapabilityRuntime {
                harness: command_name.clone(),
                profile_id,
                protocol,
                platform: platform.clone(),
                supported_actions: supported,
                configured,
                launcher_observed: detections
                    .get(&command_name)
                    .is_some_and(|detection| detection.detected),
                qualified_actions: Vec::new(),
                enabled_actions: enabled,
                unavailable_reason: reason,
            }
        })
        .collect();
    CapabilityCatalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        control_contract_id: format!("vibemux.control.v{CONTROL_PROTOCOL_VERSION}"),
        protocol_version: CONTROL_PROTOCOL_VERSION,
        operations,
        runtimes,
    }
}

pub(super) async fn dispatch(
    state: &ServerState,
    operation: ControlOperation,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    if operation != ControlOperation::Capabilities || argument.is_some() {
        return Err(ControlError::InvalidRequest);
    }
    let routes = state.dispatch.catalog().unwrap_or_default();
    let probe_cache_path = state.probe_cache_path.clone();
    let catalog = tokio::task::spawn_blocking(move || build_catalog(routes, &probe_cache_path))
        .await
        .map_err(|_| ControlError::ServerTerminated)?;
    let payload = ControlPayload::Capabilities(Box::new(catalog));
    let encoded = serde_json::to_vec(&payload).map_err(|_| ControlError::InvalidFrame)?;
    if encoded.len() > MAX_CONTROL_FRAME_BYTES.saturating_sub(ENVELOPE_HEADROOM_BYTES) {
        return Err(ControlError::InvalidFrame);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_harness::AgentKind;
    use vibemux_harness::dispatch::NativeProtocol;

    fn routes() -> Vec<DispatchCatalogEntry> {
        vec![
            DispatchCatalogEntry {
                harness: AgentKind::Claude,
                protocol: NativeProtocol::ClaudeStreamJson,
                enabled: true,
                allow_execution: true,
                probe_supported: true,
            },
            DispatchCatalogEntry {
                harness: AgentKind::OpenCode,
                protocol: NativeProtocol::Acp,
                enabled: false,
                allow_execution: false,
                probe_supported: true,
            },
        ]
    }

    #[test]
    fn catalog_lists_every_operation_once_and_marks_capabilities_read_only() {
        let catalog = build_catalog(Vec::new(), Path::new("/nonexistent/probe-cache.json"));
        assert_eq!(catalog.schema_version, 1);
        assert_eq!(catalog.protocol_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(
            catalog.control_contract_id,
            format!("vibemux.control.v{CONTROL_PROTOCOL_VERSION}")
        );
        assert_eq!(catalog.operations.len(), ControlOperation::ALL.len());
        assert_eq!(ControlOperation::ALL.len(), 48);
        let mut names: Vec<&str> = catalog
            .operations
            .iter()
            .map(|operation| operation.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), catalog.operations.len());
        let capabilities = catalog
            .operations
            .iter()
            .find(|operation| operation.name == "capabilities")
            .expect("capabilities is registered");
        assert_eq!(capabilities.minimum_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(capabilities.access, CapabilityAccess::Read);
        for operation in &catalog.operations {
            assert!(!operation.name.is_empty());
            assert!(operation.minimum_version >= 1);
            assert!(!operation.handler.is_empty());
        }
    }

    #[test]
    fn runtimes_separate_support_configuration_and_enablement() {
        let catalog = build_catalog(routes(), Path::new("/nonexistent/probe-cache.json"));
        assert_eq!(catalog.runtimes.len(), AgentKind::all().len());
        let claude = catalog
            .runtimes
            .iter()
            .find(|runtime| runtime.harness == "claude")
            .expect("claude row");
        assert!(claude.configured);
        assert_eq!(claude.protocol, "claude_stream_json");
        assert_eq!(claude.profile_id, "claude.claude_stream_json");
        assert_eq!(claude.supported_actions, ["probe", "submit"]);
        assert_eq!(claude.enabled_actions, ["probe", "submit"]);
        assert_eq!(claude.qualified_actions, Vec::<String>::new());
        assert_eq!(claude.unavailable_reason, None);
        assert!(!claude.launcher_observed);

        let opencode = catalog
            .runtimes
            .iter()
            .find(|runtime| runtime.harness == "opencode")
            .expect("opencode row");
        assert!(opencode.configured);
        assert_eq!(opencode.supported_actions, ["probe"]);
        assert!(opencode.enabled_actions.is_empty());
        assert_eq!(
            opencode.unavailable_reason.as_deref(),
            Some("route_disabled")
        );

        let qwen = catalog
            .runtimes
            .iter()
            .find(|runtime| runtime.harness == "qwen")
            .expect("qwen row");
        assert!(!qwen.configured);
        assert!(qwen.supported_actions.is_empty());
        assert_eq!(
            qwen.unavailable_reason.as_deref(),
            Some("no_structured_adapter")
        );
    }

    #[test]
    fn catalog_is_content_free_and_fits_the_frame() {
        let catalog = build_catalog(routes(), Path::new("/nonexistent/probe-cache.json"));
        let encoded = serde_json::to_vec(&catalog).expect("encode catalog");
        assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES - ENVELOPE_HEADROOM_BYTES);
        let text = String::from_utf8(encoded).expect("utf8");
        // Operation names such as `dsh_session_prompt` are stable metadata;
        // no prompt text, credential, endpoint, or filesystem path is present.
        for forbidden in ["token", "endpoint", "project_root", "/Users/", ".vibemux"] {
            assert!(
                !text.contains(forbidden),
                "catalog leaked forbidden text: {forbidden}"
            );
        }
    }
}
