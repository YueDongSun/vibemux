use crate::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR, PluginProtocolError,
    wire::{CoreHello, Envelope, envelope, validate_envelope},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreSessionPhase {
    AwaitHello,
    NeedCoreHello,
    AwaitReady,
    Active,
    Draining,
    ShutdownSent,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoreSessionLifecycle {
    phase: CoreSessionPhase,
    session_id: Option<String>,
}

impl CoreSessionLifecycle {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            phase: CoreSessionPhase::AwaitHello,
            session_id: None,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> CoreSessionPhase {
        self.phase
    }

    pub fn receive(&mut self, envelope: &Envelope) -> Result<(), PluginProtocolError> {
        validate_envelope(envelope)?;
        let body = envelope
            .body
            .as_ref()
            .ok_or(PluginProtocolError::MissingBody)?;
        let next = match (self.phase, body) {
            (CoreSessionPhase::AwaitHello, envelope::Body::Hello(_)) => {
                CoreSessionPhase::NeedCoreHello
            }
            (CoreSessionPhase::AwaitReady, envelope::Body::Ready(ready))
                if self.session_id.as_deref() == Some(ready.session_id.as_str()) =>
            {
                CoreSessionPhase::Active
            }
            (CoreSessionPhase::Active, body) if is_active_traffic(body) => CoreSessionPhase::Active,
            (CoreSessionPhase::Draining, envelope::Body::Shutdown(_))
            | (CoreSessionPhase::ShutdownSent, envelope::Body::Shutdown(_)) => {
                CoreSessionPhase::Closed
            }
            (CoreSessionPhase::Draining, body) if is_drain_traffic(body) => {
                CoreSessionPhase::Draining
            }
            _ => return Err(PluginProtocolError::InvalidTransition),
        };
        self.phase = next;
        Ok(())
    }

    pub fn mark_core_hello_sent(
        &mut self,
        core_hello: &CoreHello,
    ) -> Result<(), PluginProtocolError> {
        if self.phase != CoreSessionPhase::NeedCoreHello {
            return Err(PluginProtocolError::InvalidTransition);
        }
        validate_envelope(&Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: "core_hello".to_string(),
            correlation_id: None,
            causation_id: None,
            body: Some(envelope::Body::CoreHello(core_hello.clone())),
        })?;
        self.session_id = Some(core_hello.session_id.clone());
        self.phase = CoreSessionPhase::AwaitReady;
        Ok(())
    }

    pub fn begin_drain(&mut self) -> Result<(), PluginProtocolError> {
        if self.phase != CoreSessionPhase::Active {
            return Err(PluginProtocolError::InvalidTransition);
        }
        self.phase = CoreSessionPhase::Draining;
        Ok(())
    }

    pub fn mark_shutdown_sent(&mut self) -> Result<(), PluginProtocolError> {
        if self.phase != CoreSessionPhase::Draining {
            return Err(PluginProtocolError::InvalidTransition);
        }
        self.phase = CoreSessionPhase::ShutdownSent;
        Ok(())
    }

    pub fn mark_closed(&mut self) {
        self.phase = CoreSessionPhase::Closed;
    }
}

impl Default for CoreSessionLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

fn is_active_traffic(body: &envelope::Body) -> bool {
    matches!(
        body,
        envelope::Body::Request(_)
            | envelope::Body::Response(_)
            | envelope::Body::Event(_)
            | envelope::Body::Heartbeat(_)
            | envelope::Body::Cancel(_)
            | envelope::Body::ProtocolError(_)
    )
}

fn is_drain_traffic(body: &envelope::Body) -> bool {
    matches!(
        body,
        envelope::Body::Response(_)
            | envelope::Body::Event(_)
            | envelope::Body::Heartbeat(_)
            | envelope::Body::ProtocolError(_)
    )
}

#[cfg(test)]
mod tests {
    use crate::{PROTOCOL_MAJOR, PROTOCOL_MINOR, wire::*};

    use super::*;

    fn envelope(message_id: &str, body: envelope::Body) -> Envelope {
        Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: message_id.to_string(),
            correlation_id: None,
            causation_id: None,
            body: Some(body),
        }
    }

    fn hello() -> Envelope {
        envelope(
            "hello_1",
            envelope::Body::Hello(Hello {
                plugin_id: "mock.harness".to_string(),
                plugin_version: "1.0.0".to_string(),
                kind: PluginKind::Harness.into(),
                capabilities: vec![],
                requested_permissions: vec![],
                minimum_protocol_minor: 0,
                maximum_protocol_minor: 0,
            }),
        )
    }

    fn core_hello() -> CoreHello {
        CoreHello {
            session_id: "session_1".to_string(),
            accepted_protocol_minor: PROTOCOL_MINOR,
            granted_capabilities: vec![],
            granted_permissions: vec![],
            maximum_frame_bytes: 1024,
            maximum_in_flight: 8,
            heartbeat_interval_ms: 1000,
        }
    }

    #[test]
    fn complete_handshake_active_drain_and_shutdown_sequence() {
        let mut lifecycle = CoreSessionLifecycle::new();
        lifecycle.receive(&hello()).expect("hello");
        assert_eq!(lifecycle.phase(), CoreSessionPhase::NeedCoreHello);
        lifecycle
            .mark_core_hello_sent(&core_hello())
            .expect("core hello");
        lifecycle
            .receive(&envelope(
                "ready_1",
                envelope::Body::Ready(Ready {
                    session_id: "session_1".to_string(),
                }),
            ))
            .expect("ready");
        assert_eq!(lifecycle.phase(), CoreSessionPhase::Active);
        lifecycle
            .receive(&envelope(
                "heartbeat_1",
                envelope::Body::Heartbeat(Heartbeat { sequence: 1 }),
            ))
            .expect("heartbeat");
        lifecycle.begin_drain().expect("drain");
        lifecycle.mark_shutdown_sent().expect("shutdown sent");
        lifecycle
            .receive(&envelope(
                "shutdown_1",
                envelope::Body::Shutdown(Shutdown {
                    reason_code: "core:shutdown".to_string(),
                }),
            ))
            .expect("shutdown acknowledgement");
        assert_eq!(lifecycle.phase(), CoreSessionPhase::Closed);
    }

    #[test]
    fn duplicate_and_out_of_phase_messages_do_not_change_state() {
        let mut lifecycle = CoreSessionLifecycle::new();
        let ready = envelope(
            "ready_1",
            envelope::Body::Ready(Ready {
                session_id: "session_1".to_string(),
            }),
        );
        assert_eq!(
            lifecycle.receive(&ready).expect_err("ready before hello"),
            PluginProtocolError::InvalidTransition
        );
        assert_eq!(lifecycle.phase(), CoreSessionPhase::AwaitHello);
        lifecycle.receive(&hello()).expect("hello");
        assert_eq!(
            lifecycle.receive(&hello()).expect_err("duplicate hello"),
            PluginProtocolError::InvalidTransition
        );
        assert_eq!(lifecycle.phase(), CoreSessionPhase::NeedCoreHello);

        lifecycle
            .mark_core_hello_sent(&core_hello())
            .expect("core hello");
        let wrong_session = envelope(
            "ready_wrong",
            envelope::Body::Ready(Ready {
                session_id: "session_2".to_string(),
            }),
        );
        assert_eq!(
            lifecycle
                .receive(&wrong_session)
                .expect_err("wrong session ready"),
            PluginProtocolError::InvalidTransition
        );
        assert_eq!(lifecycle.phase(), CoreSessionPhase::AwaitReady);
    }
}
