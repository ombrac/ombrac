use serde::{Deserialize, Serialize};
pub use tokio_util::codec::LengthDelimitedCodec;

use crate::protocol::{ClientConnect, ClientHello, ServerConnectResponse};

/// Maximum frame length for length-delimited codec [8MB]
pub const MAX_FRAME_LENGTH: usize = 8 * 1024 * 1024;

/// Messages sent from client to server.
///
/// These messages are sent over the control stream during authentication
/// or over data streams for connection establishment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Initial authentication message containing credentials.
    Hello(ClientHello),
    /// Connection request to establish a tunnel to a destination address.
    Connect(ClientConnect),
}

/// Messages sent from server to client.
///
/// These messages are responses to client requests, sent over
/// the same stream as the corresponding request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Response to a connection request, indicating success or failure.
    ConnectResponse(ServerConnectResponse),
}

/// Creates a length-delimited codec for framing protocol messages.
///
/// The codec uses a 4-byte big-endian length prefix, followed by the message payload.
/// Maximum frame size is limited to prevent memory exhaustion attacks.
pub fn length_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .length_field_offset(0)
        .length_field_length(4)
        .length_adjustment(0)
        .num_skip(4)
        .max_frame_length(MAX_FRAME_LENGTH)
        .new_codec()
}

/// Length prefix size in bytes (u32 = 4 bytes)
pub const LENGTH_PREFIX_SIZE: usize = 4;

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::{Decoder, Encoder};

    use super::*;
    use crate::protocol::{
        ClientHello, ConnectErrorKind, ServerConnectResponse, PROTOCOL_VERSION, decode, encode,
    };

    // ── Group G: length_codec() encoder / decoder ────────────────────────────

    #[test]
    fn test_length_codec_encode_decode_roundtrip() {
        let mut codec = length_codec();
        let mut buf = BytesMut::new();
        Encoder::<Bytes>::encode(&mut codec, Bytes::from_static(b"hello world"), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.as_ref(), b"hello world");
    }

    #[test]
    fn test_length_codec_encode_decode_empty_frame() {
        let mut codec = length_codec();
        let mut buf = BytesMut::new();
        Encoder::<Bytes>::encode(&mut codec, Bytes::new(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_length_codec_partial_data_returns_none() {
        let mut codec = length_codec();
        let mut buf = BytesMut::from(&[0u8, 0][..]);
        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_length_codec_multi_frame_sequence() {
        let mut codec = length_codec();
        let mut buf = BytesMut::new();
        Encoder::<Bytes>::encode(&mut codec, Bytes::from_static(b"first"), &mut buf).unwrap();
        Encoder::<Bytes>::encode(&mut codec, Bytes::from_static(b"second"), &mut buf).unwrap();
        let first = codec.decode(&mut buf).unwrap().unwrap();
        let second = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(first.as_ref(), b"first");
        assert_eq!(second.as_ref(), b"second");
    }

    #[test]
    fn test_length_codec_frame_at_exact_max_size() {
        let mut codec = length_codec();
        let mut buf = BytesMut::new();
        let payload = Bytes::from(vec![0u8; MAX_FRAME_LENGTH]);
        Encoder::<Bytes>::encode(&mut codec, payload.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.len(), MAX_FRAME_LENGTH);
    }

    #[test]
    fn test_length_codec_frame_exceeds_max_size_rejected() {
        let mut codec = length_codec();
        // Craft a 4-byte big-endian length prefix exceeding MAX_FRAME_LENGTH
        let oversized_len = (MAX_FRAME_LENGTH + 1) as u32;
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&oversized_len.to_be_bytes());
        buf.extend_from_slice(&[0u8; 16]); // some dummy bytes after the prefix
        let result = codec.decode(&mut buf);
        assert!(result.is_err(), "expected error for oversized frame");
    }

    #[test]
    fn test_length_codec_length_prefix_is_4_bytes() {
        let mut codec = length_codec();
        let mut buf = BytesMut::new();
        let payload = Bytes::from(vec![99u8; 16]);
        Encoder::<Bytes>::encode(&mut codec, payload, &mut buf).unwrap();
        assert_eq!(buf.len(), LENGTH_PREFIX_SIZE + 16);
    }

    // ── Group H: ClientMessage / ServerMessage roundtrips ────────────────────

    #[test]
    fn test_client_message_hello_roundtrip() {
        let msg = ClientMessage::Hello(ClientHello {
            version: PROTOCOL_VERSION,
            secret: [1u8; 32],
            options: Bytes::new(),
        });
        let bytes = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_server_message_connect_response_roundtrip() {
        // Ok variant
        let ok = ServerMessage::ConnectResponse(ServerConnectResponse::Ok);
        let bytes = encode(&ok).unwrap();
        let decoded: ServerMessage = decode(&bytes).unwrap();
        assert_eq!(decoded, ok);

        // Err variant
        let err_msg = ServerMessage::ConnectResponse(ServerConnectResponse::Err {
            kind: ConnectErrorKind::TimedOut,
            message: "timed out".to_string(),
        });
        let bytes = encode(&err_msg).unwrap();
        let decoded: ServerMessage = decode(&bytes).unwrap();
        assert_eq!(decoded, err_msg);
    }
}
