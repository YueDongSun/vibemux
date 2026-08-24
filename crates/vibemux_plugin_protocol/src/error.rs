use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PluginProtocolError {
    #[error("plugin frame limit is invalid")]
    InvalidFrameLimit,
    #[error("plugin frame length is zero")]
    ZeroFrame,
    #[error("plugin frame exceeds the configured limit")]
    FrameTooLarge,
    #[error("plugin frame is truncated or has trailing bytes")]
    InvalidFrameLength,
    #[error("plugin frame I/O failed")]
    FrameIo,
    #[error("plugin protobuf encoding failed")]
    EncodeFailed,
    #[error("plugin protobuf decoding failed")]
    DecodeFailed,
    #[error("plugin protocol major version is unsupported")]
    UnsupportedMajor,
    #[error("plugin protocol minor version is unsupported")]
    UnsupportedMinor,
    #[error("plugin envelope body is missing")]
    MissingBody,
    #[error("plugin protocol identifier is invalid")]
    InvalidIdentifier,
    #[error("plugin message fields are invalid")]
    InvalidMessage,
    #[error("plugin collection exceeds its bound")]
    CollectionTooLarge,
    #[error("plugin collection contains duplicate identifiers")]
    DuplicateIdentifier,
    #[error("plugin manifest is invalid")]
    InvalidManifest,
    #[error("plugin manifest is too large")]
    ManifestTooLarge,
    #[error("plugin negotiation does not match the manifest or core policy")]
    NegotiationRejected,
    #[error("plugin lifecycle transition is invalid")]
    InvalidTransition,
}

impl PluginProtocolError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidFrameLimit => "plugin_invalid_frame_limit",
            Self::ZeroFrame => "plugin_zero_frame",
            Self::FrameTooLarge => "plugin_frame_too_large",
            Self::InvalidFrameLength => "plugin_invalid_frame_length",
            Self::FrameIo => "plugin_frame_io",
            Self::EncodeFailed => "plugin_encode_failed",
            Self::DecodeFailed => "plugin_decode_failed",
            Self::UnsupportedMajor => "plugin_unsupported_major",
            Self::UnsupportedMinor => "plugin_unsupported_minor",
            Self::MissingBody => "plugin_missing_body",
            Self::InvalidIdentifier => "plugin_invalid_identifier",
            Self::InvalidMessage => "plugin_invalid_message",
            Self::CollectionTooLarge => "plugin_collection_too_large",
            Self::DuplicateIdentifier => "plugin_duplicate_identifier",
            Self::InvalidManifest => "plugin_invalid_manifest",
            Self::ManifestTooLarge => "plugin_manifest_too_large",
            Self::NegotiationRejected => "plugin_negotiation_rejected",
            Self::InvalidTransition => "plugin_invalid_transition",
        }
    }
}
