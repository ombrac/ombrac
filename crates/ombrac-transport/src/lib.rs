use std::error::Error;
use std::future::Future;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(feature = "quic")]
pub mod quic;

pub trait Transport: Send + Sync + 'static {
    fn reliable(&self) -> impl Future<Output = Result<impl Reliable>> + Send;

    #[cfg(feature = "datagram")]
    fn unreliable(&self) -> impl Future<Output = Result<impl Unreliable>> + Send;
}

#[cfg(feature = "datagram")]
pub trait Unreliable: Send + Sync + 'static {
    fn send(&self, data: bytes::Bytes) -> impl Future<Output = Result<()>> + Send;
    fn recv(&self) -> impl Future<Output = Result<bytes::Bytes>> + Send;
}

pub trait Reliable: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
