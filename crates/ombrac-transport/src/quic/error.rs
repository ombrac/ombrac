use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Rustls(#[from] rustls::Error),

    #[error(transparent)]
    Rcgen(#[from] rcgen::Error),

    #[error(transparent)]
    QuinnRustls(#[from] quinn::crypto::rustls::NoInitialCipherSuite),

    #[error(transparent)]
    QuinnConnect(#[from] quinn::ConnectError),

    #[error(transparent)]
    QuinnConnection(#[from] quinn::ConnectionError),

    #[cfg(feature = "datagram")]
    #[error(transparent)]
    QuinnSendDatagram(#[from] quinn::SendDatagramError),

    #[error(transparent)]
    QuinnVarInt(#[from] quinn::VarIntBoundsExceeded),

    #[error("Certificate or Key must be provided when TLS is enabled")]
    ServerMissingCertificate,

    #[error("Invalid congestion algorithm")]
    InvalidCongestion,

    #[error("ConnectionClosed")]
    ConnectionClosed,
}

impl From<Error> for io::Error {
    fn from(err: Error) -> Self {
        match err {
            Error::Io(e) => e,
            _ => io::Error::other(err),
        }
    }
}
