use std::collections::BTreeSet;

use crate::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR, PluginProtocolError,
    frame::FrameCodecConfig,
    limits::{
        DEFAULT_HEARTBEAT_INTERVAL_MS, DEFAULT_MAXIMUM_IN_FLIGHT, HARD_MAX_PLUGIN_FRAME_BYTES,
        HARD_MAXIMUM_IN_FLIGHT, MAXIMUM_HEARTBEAT_INTERVAL_MS, MINIMUM_HEARTBEAT_INTERVAL_MS,
    },
    manifest::{PluginManifest, SupportedPlatform},
    wire::{
        CoreHello, Envelope, Hello, envelope, validate_envelope, validate_identifier,
        validate_policy_identifiers,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorePluginPolicy {
    allowed_capabilities: BTreeSet<String>,
    allowed_permissions: BTreeSet<String>,
    frame_config: FrameCodecConfig,
    maximum_in_flight: u32,
    heartbeat_interval_ms: u64,
    platform: SupportedPlatform,
}

impl CorePluginPolicy {
    pub fn new(
        allowed_capabilities: Vec<String>,
        allowed_permissions: Vec<String>,
        frame_config: FrameCodecConfig,
        maximum_in_flight: u32,
        heartbeat_interval_ms: u64,
        platform: SupportedPlatform,
    ) -> Result<Self, PluginProtocolError> {
        validate_policy_identifiers(&allowed_capabilities)?;
        validate_policy_identifiers(&allowed_permissions)?;
        if maximum_in_flight == 0
            || maximum_in_flight > HARD_MAXIMUM_IN_FLIGHT
            || !(MINIMUM_HEARTBEAT_INTERVAL_MS..=MAXIMUM_HEARTBEAT_INTERVAL_MS)
                .contains(&heartbeat_interval_ms)
        {
            return Err(PluginProtocolError::NegotiationRejected);
        }
        Ok(Self {
            allowed_capabilities: allowed_capabilities.into_iter().collect(),
            allowed_permissions: allowed_permissions.into_iter().collect(),
            frame_config,
            maximum_in_flight,
            heartbeat_interval_ms,
            platform,
        })
    }
}

impl Default for CorePluginPolicy {
    fn default() -> Self {
        Self {
            allowed_capabilities: BTreeSet::new(),
            allowed_permissions: BTreeSet::new(),
            frame_config: FrameCodecConfig::default(),
            maximum_in_flight: DEFAULT_MAXIMUM_IN_FLIGHT,
            heartbeat_interval_ms: DEFAULT_HEARTBEAT_INTERVAL_MS,
            platform: current_platform(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NegotiatedSession {
    pub accepted_protocol_minor: u32,
    pub granted_capabilities: Vec<String>,
    pub granted_permissions: Vec<String>,
    pub core_hello: CoreHello,
}

pub fn negotiate_hello(
    manifest: &PluginManifest,
    hello: &Hello,
    session_id: &str,
    policy: &CorePluginPolicy,
) -> Result<NegotiatedSession, PluginProtocolError> {
    manifest.validate()?;
    validate_identifier(session_id)?;
    validate_envelope(&Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id: "negotiation_hello".to_string(),
        correlation_id: None,
        causation_id: None,
        body: Some(envelope::Body::Hello(hello.clone())),
    })?;
    let manifest_kind: crate::wire::PluginKind = manifest.kind.into();
    if hello.plugin_id != manifest.plugin_id
        || hello.plugin_version != manifest.plugin_version
        || hello.kind != i32::from(manifest_kind)
        || manifest.protocol_major != PROTOCOL_MAJOR
        || !(manifest.minimum_protocol_minor..=manifest.maximum_protocol_minor)
            .contains(&PROTOCOL_MINOR)
        || !(hello.minimum_protocol_minor..=hello.maximum_protocol_minor).contains(&PROTOCOL_MINOR)
        || !manifest.supported_platforms.contains(&policy.platform)
    {
        return Err(PluginProtocolError::NegotiationRejected);
    }

    let manifest_capabilities = manifest
        .capabilities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let manifest_permissions = manifest
        .requested_permissions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if hello
        .capabilities
        .iter()
        .any(|capability| !manifest_capabilities.contains(capability.as_str()))
        || hello
            .requested_permissions
            .iter()
            .any(|permission| !manifest_permissions.contains(permission.as_str()))
    {
        return Err(PluginProtocolError::NegotiationRejected);
    }

    let granted_capabilities = hello
        .capabilities
        .iter()
        .filter(|capability| policy.allowed_capabilities.contains(capability.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let granted_permissions = hello
        .requested_permissions
        .iter()
        .filter(|permission| policy.allowed_permissions.contains(permission.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let maximum_frame_bytes = u32::try_from(policy.frame_config.maximum_frame_bytes())
        .map_err(|_| PluginProtocolError::NegotiationRejected)?;
    if maximum_frame_bytes as usize > HARD_MAX_PLUGIN_FRAME_BYTES {
        return Err(PluginProtocolError::NegotiationRejected);
    }
    let core_hello = CoreHello {
        session_id: session_id.to_string(),
        accepted_protocol_minor: PROTOCOL_MINOR,
        granted_capabilities: granted_capabilities.clone(),
        granted_permissions: granted_permissions.clone(),
        maximum_frame_bytes,
        maximum_in_flight: policy.maximum_in_flight,
        heartbeat_interval_ms: policy.heartbeat_interval_ms,
    };
    Ok(NegotiatedSession {
        accepted_protocol_minor: PROTOCOL_MINOR,
        granted_capabilities,
        granted_permissions,
        core_hello,
    })
}

const fn current_platform() -> SupportedPlatform {
    #[cfg(windows)]
    {
        SupportedPlatform::Windows
    }
    #[cfg(target_os = "linux")]
    {
        SupportedPlatform::Linux
    }
    #[cfg(target_os = "macos")]
    {
        SupportedPlatform::Macos
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        manifest::{ManifestPluginKind, SupportedPlatform},
        wire::PluginKind,
    };

    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            plugin_id: "mock.harness".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: ManifestPluginKind::Harness,
            entry_point: vec!["mock_plugin".to_string()],
            capabilities: vec!["harness:cancel".to_string(), "harness:run".to_string()],
            requested_permissions: vec![
                "network:connect".to_string(),
                "workspace:read".to_string(),
            ],
            supported_platforms: vec![
                SupportedPlatform::Windows,
                SupportedPlatform::Linux,
                SupportedPlatform::Macos,
            ],
            protocol_major: PROTOCOL_MAJOR,
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }
    }

    fn hello() -> Hello {
        Hello {
            plugin_id: "mock.harness".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: PluginKind::Harness.into(),
            capabilities: vec!["harness:run".to_string(), "harness:cancel".to_string()],
            requested_permissions: vec![
                "workspace:read".to_string(),
                "network:connect".to_string(),
            ],
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }
    }

    #[test]
    fn grants_are_deterministic_policy_intersections() {
        let policy = CorePluginPolicy::new(
            vec!["harness:run".to_string()],
            vec!["workspace:read".to_string()],
            FrameCodecConfig::default(),
            8,
            1000,
            SupportedPlatform::Windows,
        )
        .expect("policy");
        let negotiated =
            negotiate_hello(&manifest(), &hello(), "session_1", &policy).expect("negotiation");
        assert_eq!(negotiated.granted_capabilities, ["harness:run"]);
        assert_eq!(negotiated.granted_permissions, ["workspace:read"]);
        assert!(
            !negotiated
                .core_hello
                .granted_permissions
                .contains(&"network:connect".to_string())
        );
    }

    #[test]
    fn undeclared_capability_and_identity_mismatch_are_rejected() {
        let mut undeclared_hello = hello();
        undeclared_hello
            .capabilities
            .push("terminal:write".to_string());
        assert_eq!(
            negotiate_hello(
                &manifest(),
                &undeclared_hello,
                "session_1",
                &CorePluginPolicy::default()
            )
            .expect_err("undeclared capability"),
            PluginProtocolError::NegotiationRejected
        );

        let mut identity_hello = hello();
        identity_hello.plugin_id = "other.harness".to_string();
        assert_eq!(
            negotiate_hello(
                &manifest(),
                &identity_hello,
                "session_1",
                &CorePluginPolicy::default()
            )
            .expect_err("identity mismatch"),
            PluginProtocolError::NegotiationRejected
        );

        let mut unsupported_platform = manifest();
        unsupported_platform.supported_platforms = vec![SupportedPlatform::Macos];
        let windows_policy = CorePluginPolicy::new(
            vec![],
            vec![],
            FrameCodecConfig::default(),
            8,
            1000,
            SupportedPlatform::Windows,
        )
        .expect("Windows policy");
        assert_eq!(
            negotiate_hello(
                &unsupported_platform,
                &hello(),
                "session_1",
                &windows_policy,
            )
            .expect_err("unsupported platform"),
            PluginProtocolError::NegotiationRejected
        );
    }

    #[test]
    fn policy_numeric_limits_fail_closed() {
        assert_eq!(
            CorePluginPolicy::new(
                vec![],
                vec![],
                FrameCodecConfig::default(),
                HARD_MAXIMUM_IN_FLIGHT + 1,
                DEFAULT_HEARTBEAT_INTERVAL_MS,
                SupportedPlatform::Windows,
            )
            .expect_err("in-flight hard limit"),
            PluginProtocolError::NegotiationRejected
        );
        assert_eq!(
            CorePluginPolicy::new(
                vec![],
                vec![],
                FrameCodecConfig::default(),
                1,
                MINIMUM_HEARTBEAT_INTERVAL_MS - 1,
                SupportedPlatform::Windows,
            )
            .expect_err("heartbeat minimum"),
            PluginProtocolError::NegotiationRejected
        );
    }
}
