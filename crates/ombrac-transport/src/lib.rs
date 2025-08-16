use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(feature = "datagram")]
use bytes::Bytes;

#[cfg(feature = "quic")]
pub mod quic;

pub trait Initiator: Send + Sync + 'static {
    fn open_bidirectional(&self) -> impl Future<Output = io::Result<impl Reliable>> + Send;
    #[cfg(feature = "datagram")]
    fn open_datagram(&self) -> impl Future<Output = io::Result<impl Unreliable>> + Send;
}

pub trait Acceptor: Send + Sync + 'static {
    fn accept_bidirectional(&self) -> impl Future<Output = io::Result<impl Reliable>> + Send;
    #[cfg(feature = "datagram")]
    fn accept_datagram(&self) -> impl Future<Output = io::Result<impl Unreliable>> + Send;
}

pub trait Reliable: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static {}

#[cfg(feature = "datagram")]
pub trait Unreliable: Send + Sync + 'static {
    fn send(&self, data: Bytes) -> impl Future<Output = io::Result<()>> + Send;
    fn recv(&self) -> impl Future<Output = io::Result<Bytes>> + Send;
}
