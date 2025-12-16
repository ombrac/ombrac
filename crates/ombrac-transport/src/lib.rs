use std::future::Future;
use std::io::Result;
use std::net::SocketAddr;

use auto_impl::auto_impl;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod io;
#[cfg(feature = "quic")]
pub mod quic;

#[auto_impl(&, Arc, Box)]
pub trait Initiator: Send + Sync + 'static {
    type Connection: Connection;

    fn local_addr(&self) -> Result<SocketAddr>;
    fn rebind(&self) -> impl Future<Output = Result<()>> + Send;
    fn connect(&self) -> impl Future<Output = Result<Self::Connection>> + Send;
}

#[auto_impl(&, Arc, Box)]
pub trait Acceptor: Send + Sync + 'static {
    type Connection: Connection;

    fn local_addr(&self) -> Result<SocketAddr>;
    fn accept(&self) -> impl Future<Output = Result<Self::Connection>> + Send;
}

#[auto_impl(&, Arc, Box)]
pub trait Connection: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync;
    fn id(&self) -> usize;
    fn close(&self, error_code: u32, reason: &[u8]);
    fn remote_address(&self) -> Result<SocketAddr>;

    fn open_bidirectional(&self) -> impl Future<Output = Result<Self::Stream>> + Send;
    fn accept_bidirectional(&self) -> impl Future<Output = Result<Self::Stream>> + Send;

    #[cfg(feature = "datagram")]
    fn max_datagram_size(&self) -> Option<usize>;
    #[cfg(feature = "datagram")]
    fn send_datagram(&self, data: bytes::Bytes) -> impl Future<Output = Result<()>> + Send;
    #[cfg(feature = "datagram")]
    fn read_datagram(&self) -> impl Future<Output = Result<bytes::Bytes>> + Send;
}
