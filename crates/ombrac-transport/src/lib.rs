use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(feature = "datagram")]
use bytes::Bytes;

#[cfg(feature = "quic")]
pub mod quic;

#[cfg(feature = "datagram")]
pub trait Unreliable: Send + Sync + 'static {
    fn send(&self, data: Bytes) -> impl Future<Output = io::Result<()>> + Send;
    fn recv(&self) -> impl Future<Output = io::Result<Bytes>> + Send;
}

pub trait Reliable: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

pub trait Initiator: Send + Sync + 'static {
    #[cfg(feature = "datagram")]
    fn open_datagram(&self) -> impl Future<Output = io::Result<impl Unreliable>> + Send;
    fn open_bidirectional(&self) -> impl Future<Output = io::Result<impl Reliable>> + Send;
}

pub trait Acceptor: Send + Sync + 'static {
    #[cfg(feature = "datagram")]
    fn accept_datagram(&self) -> impl Future<Output = io::Result<impl Unreliable>> + Send;
    fn accept_bidirectional(&self) -> impl Future<Output = io::Result<impl Reliable>> + Send;
}
