use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Rustls error: {0}")]
    Rustls(#[from] rustls::Error),

    #[error("Rcgen error: {0}")]
    Rcgen(#[from] rcgen::Error),

    #[error("Rustls error: {0}")]
    QuinnRustls(#[from] quinn::crypto::rustls::NoInitialCipherSuite),

    #[error("QUIC connect error: {0}")]
    QuinnConnect(#[from] quinn::ConnectError),

    #[error("Rustls error: {0}")]
    QuinnVarInt(#[from] quinn::VarIntBoundsExceeded),

    #[error("Certificate or Key must be provided when TLS is enabled")]
    ServerMissingCertificate,

    #[error("Invalid congestion algorithm")]
    InvalidCongestion,
}

impl From<Error> for io::Error {
    fn from(err: Error) -> Self {
        match err {
            Error::Io(e) => e,
            _ => io::Error::other(err),
        }
    }
}
