#[cfg(feature = "transport-quic")]
pub mod quic;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
