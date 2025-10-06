use serde::{Deserialize, Serialize};
pub use tokio_util::codec::LengthDelimitedCodec;

use crate::protocol::{ClientConnect, ClientHello, HandshakeError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpstreamMessage {
    Hello(ClientHello),
    Connect(ClientConnect),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ServerHandshakeResponse {
    Ok,
    Err(HandshakeError),
}

pub fn length_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .length_field_offset(0)
        .length_field_length(4)
        .length_adjustment(0)
        .num_skip(4)
        .max_frame_length(8 * 1024 * 1024)
        .new_codec()
}
