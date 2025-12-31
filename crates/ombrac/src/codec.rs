use serde::{Deserialize, Serialize};
pub use tokio_util::codec::LengthDelimitedCodec;

use crate::protocol::{ClientConnect, ClientHello, ServerConnectResponse};

/// Maximum frame length for length-delimited codec [8MB]
pub const MAX_FRAME_LENGTH: usize = 8 * 1024 * 1024;

/// Messages sent from client to server.
///
/// These messages are sent over the control stream during handshake
/// or over data streams for connection establishment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Initial handshake message containing authentication credentials.
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
