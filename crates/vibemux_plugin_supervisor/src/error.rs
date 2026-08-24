use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PluginSupervisorError {
    #[error("plugin supervisor configuration is invalid")]
    InvalidConfiguration,
    #[error("plugin launch specification is invalid")]
    InvalidLaunch,
    #[error("plugin process could not be spawned")]
    SpawnFailed,
    #[error("plugin process pipe is unavailable")]
    MissingPipe,
    #[error("plugin handshake exceeded its deadline")]
    HandshakeTimeout,
    #[error("plugin handshake was rejected")]
    HandshakeRejected,
    #[error("plugin protocol failed: {code}")]
    Protocol { code: String },
    #[error("plugin outbound queue is full")]
    QueueFull,
    #[error("plugin transport queue is closed")]
    QueueClosed,
    #[error("plugin receive exceeded its deadline")]
    ReceiveTimeout,
    #[error("plugin session is closed")]
    SessionClosed,
    #[error("plugin shutdown exceeded its deadline")]
    ShutdownTimeout,
    #[error("plugin child status is unavailable")]
    ChildStatusUnavailable,
}

impl PluginSupervisorError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidConfiguration => "plugin_supervisor_invalid_configuration",
            Self::InvalidLaunch => "plugin_supervisor_invalid_launch",
            Self::SpawnFailed => "plugin_supervisor_spawn_failed",
            Self::MissingPipe => "plugin_supervisor_missing_pipe",
            Self::HandshakeTimeout => "plugin_supervisor_handshake_timeout",
            Self::HandshakeRejected => "plugin_supervisor_handshake_rejected",
            Self::Protocol { code } => code,
            Self::QueueFull => "plugin_supervisor_queue_full",
            Self::QueueClosed => "plugin_supervisor_queue_closed",
            Self::ReceiveTimeout => "plugin_supervisor_receive_timeout",
            Self::SessionClosed => "plugin_supervisor_session_closed",
            Self::ShutdownTimeout => "plugin_supervisor_shutdown_timeout",
            Self::ChildStatusUnavailable => "plugin_supervisor_child_status_unavailable",
        }
    }
}

impl From<vibemux_plugin_protocol::PluginProtocolError> for PluginSupervisorError {
    fn from(error: vibemux_plugin_protocol::PluginProtocolError) -> Self {
        Self::Protocol {
            code: error.code().to_string(),
        }
    }
}
