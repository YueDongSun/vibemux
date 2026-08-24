use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{PluginProtocolError, wire::Envelope, wire::validate_envelope};

pub use crate::limits::{DEFAULT_MAX_PLUGIN_FRAME_BYTES, HARD_MAX_PLUGIN_FRAME_BYTES};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameCodecConfig {
    maximum_frame_bytes: usize,
}

impl FrameCodecConfig {
    pub fn new(maximum_frame_bytes: usize) -> Result<Self, PluginProtocolError> {
        if maximum_frame_bytes == 0 || maximum_frame_bytes > HARD_MAX_PLUGIN_FRAME_BYTES {
            return Err(PluginProtocolError::InvalidFrameLimit);
        }
        Ok(Self {
            maximum_frame_bytes,
        })
    }

    #[must_use]
    pub const fn maximum_frame_bytes(self) -> usize {
        self.maximum_frame_bytes
    }
}

impl Default for FrameCodecConfig {
    fn default() -> Self {
        Self {
            maximum_frame_bytes: DEFAULT_MAX_PLUGIN_FRAME_BYTES,
        }
    }
}

pub fn encode_frame(
    envelope: &Envelope,
    config: FrameCodecConfig,
) -> Result<Vec<u8>, PluginProtocolError> {
    validate_envelope(envelope)?;
    let length = envelope.encoded_len();
    validate_frame_length(length, config)?;
    let length = u32::try_from(length).map_err(|_| PluginProtocolError::FrameTooLarge)?;
    let mut frame = Vec::with_capacity(length as usize + 4);
    frame.extend_from_slice(&length.to_be_bytes());
    envelope
        .encode(&mut frame)
        .map_err(|_| PluginProtocolError::EncodeFailed)?;
    Ok(frame)
}

pub fn decode_frame(
    frame: &[u8],
    config: FrameCodecConfig,
) -> Result<Envelope, PluginProtocolError> {
    if frame.len() < 4 {
        return Err(PluginProtocolError::InvalidFrameLength);
    }
    let length = usize::try_from(u32::from_be_bytes(
        frame[..4]
            .try_into()
            .map_err(|_| PluginProtocolError::InvalidFrameLength)?,
    ))
    .map_err(|_| PluginProtocolError::FrameTooLarge)?;
    validate_frame_length(length, config)?;
    if frame.len() != length + 4 {
        return Err(PluginProtocolError::InvalidFrameLength);
    }
    decode_payload(&frame[4..])
}

pub async fn read_envelope<R>(
    reader: &mut R,
    config: FrameCodecConfig,
) -> Result<Envelope, PluginProtocolError>
where
    R: AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    reader
        .read_exact(&mut prefix)
        .await
        .map_err(|_| PluginProtocolError::FrameIo)?;
    let length = usize::try_from(u32::from_be_bytes(prefix))
        .map_err(|_| PluginProtocolError::FrameTooLarge)?;
    validate_frame_length(length, config)?;
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| PluginProtocolError::FrameIo)?;
    decode_payload(&payload)
}

pub async fn write_envelope<W>(
    writer: &mut W,
    envelope: &Envelope,
    config: FrameCodecConfig,
) -> Result<(), PluginProtocolError>
where
    W: AsyncWrite + Unpin,
{
    let frame = encode_frame(envelope, config)?;
    writer
        .write_all(&frame)
        .await
        .map_err(|_| PluginProtocolError::FrameIo)?;
    writer
        .flush()
        .await
        .map_err(|_| PluginProtocolError::FrameIo)
}

fn decode_payload(payload: &[u8]) -> Result<Envelope, PluginProtocolError> {
    let envelope = Envelope::decode(payload).map_err(|_| PluginProtocolError::DecodeFailed)?;
    validate_envelope(&envelope)?;
    Ok(envelope)
}

fn validate_frame_length(
    length: usize,
    config: FrameCodecConfig,
) -> Result<(), PluginProtocolError> {
    if length == 0 {
        return Err(PluginProtocolError::ZeroFrame);
    }
    if length > config.maximum_frame_bytes {
        return Err(PluginProtocolError::FrameTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::{PROTOCOL_MAJOR, PROTOCOL_MINOR, wire};

    use super::*;

    fn heartbeat_envelope() -> Envelope {
        Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: "heartbeat_1".to_string(),
            correlation_id: Some("session_1".to_string()),
            causation_id: None,
            body: Some(wire::envelope::Body::Heartbeat(wire::Heartbeat {
                sequence: 1,
            })),
        }
    }

    #[test]
    fn canonical_frame_round_trips_and_tolerates_unknown_field() {
        let config = FrameCodecConfig::default();
        let envelope = heartbeat_envelope();
        let frame = encode_frame(&envelope, config).expect("encode frame");
        assert_eq!(
            decode_frame(&frame, config).expect("decode frame"),
            envelope
        );

        let mut payload = envelope.encode_to_vec();
        payload.extend_from_slice(&[0xa0, 0x06, 0x01]);
        let mut frame = Vec::new();
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        assert_eq!(
            decode_frame(&frame, config).expect("decode additive unknown field"),
            envelope
        );
    }

    #[test]
    fn codec_limit_is_validated() {
        assert_eq!(
            FrameCodecConfig::new(0).expect_err("zero limit"),
            PluginProtocolError::InvalidFrameLimit
        );
        assert_eq!(
            FrameCodecConfig::new(HARD_MAX_PLUGIN_FRAME_BYTES + 1).expect_err("hard limit"),
            PluginProtocolError::InvalidFrameLimit
        );
    }

    #[test]
    fn malformed_truncated_and_trailing_frames_fail_closed() {
        let config = FrameCodecConfig::default();
        assert_eq!(
            decode_frame(&[0, 0, 0], config).expect_err("short prefix"),
            PluginProtocolError::InvalidFrameLength
        );
        assert_eq!(
            decode_frame(&[0, 0, 0, 0], config).expect_err("zero frame"),
            PluginProtocolError::ZeroFrame
        );
        assert_eq!(
            decode_frame(&[0, 0, 0, 1, 0xff], config).expect_err("malformed protobuf"),
            PluginProtocolError::DecodeFailed
        );
        let mut frame = encode_frame(&heartbeat_envelope(), config).expect("frame");
        frame.push(0);
        assert_eq!(
            decode_frame(&frame, config).expect_err("trailing bytes"),
            PluginProtocolError::InvalidFrameLength
        );
    }

    #[tokio::test]
    async fn oversized_prefix_is_rejected_before_payload_read() {
        let config = FrameCodecConfig::new(32).expect("config");
        let (mut writer, mut reader) = tokio::io::duplex(4);
        writer
            .write_all(&33_u32.to_be_bytes())
            .await
            .expect("write prefix");
        assert_eq!(
            read_envelope(&mut reader, config)
                .await
                .expect_err("oversized prefix"),
            PluginProtocolError::FrameTooLarge
        );
    }

    #[tokio::test]
    async fn truncated_async_payload_is_a_bounded_io_error() {
        let config = FrameCodecConfig::new(32).expect("config");
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer
            .write_all(&10_u32.to_be_bytes())
            .await
            .expect("write prefix");
        writer
            .write_all(&[1, 2])
            .await
            .expect("write partial payload");
        writer.shutdown().await.expect("close writer");
        assert_eq!(
            read_envelope(&mut reader, config)
                .await
                .expect_err("truncated payload"),
            PluginProtocolError::FrameIo
        );
    }

    proptest! {
        #[test]
        fn every_length_above_limit_is_rejected(length in (DEFAULT_MAX_PLUGIN_FRAME_BYTES as u32 + 1)..=u32::MAX) {
            let result = validate_frame_length(length as usize, FrameCodecConfig::default());
            prop_assert_eq!(result, Err(PluginProtocolError::FrameTooLarge));
        }
    }
}
