pub mod server;

pub use server::Server;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    JoinError(#[from] tokio::task::JoinError),

    #[error("Secret does not match")]
    PermissionDenied,

    #[cfg(feature = "transport-quic")]
    #[error("{0}")]
    TransportQUIC(#[from] ombrac_transport::quic::Error),
}

type Result<T> = std::result::Result<T, Error>;
