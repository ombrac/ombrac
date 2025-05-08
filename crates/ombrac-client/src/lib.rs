mod client;

pub mod endpoint;
pub mod transport {
    #[cfg(feature = "transport-quic")]
    pub mod quic {
        pub use ombrac_transport::quic::Connection;
        pub use ombrac_transport::quic::Error;
        pub use ombrac_transport::quic::client::Builder;
    }
}

pub use client::Client;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    JoinError(#[from] tokio::task::JoinError),

    #[error("Secret does not match")]
    PermissionDenied,

    #[error("{0}")]
    Timeout(String),
}

type Result<T> = std::result::Result<T, Error>;
