use tokio::io;
use tokio::io::AsyncWriteExt;

#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;
use ombrac_transport::{Initiator, Reliable};

use crate::Secret;
use crate::address::Address;
#[cfg(feature = "datagram")]
use crate::associate::Associate;
use crate::connect::Connect;

pub struct Client<T> {
    transport: T,
}

impl<T: Initiator> Client<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub async fn connect<A: Into<Address>>(
        &self,
        target: A,
        secret: Secret,
    ) -> io::Result<Stream<impl Reliable>> {
        let mut stream = self.transport.open_bidirectional().await?;

        let request = Connect::with(secret, target);
        stream.write_all(&request.to_bytes()?).await?;

        Ok(Stream(stream))
    }

    #[cfg(feature = "datagram")]
    pub async fn associate(&self) -> io::Result<Datagram<impl Unreliable>> {
        let datagram = self.transport.open_datagram().await?;

        Ok(Datagram(datagram))
    }
}

pub struct Stream<R: Reliable>(pub(crate) R);

mod impl_async_read {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite};

    use super::{Reliable, Stream};

    impl<R: Reliable> AsyncRead for Stream<R> {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            AsyncRead::poll_read(Pin::new(&mut self.get_mut().0), cx, buf)
        }
    }

    impl<R: Reliable> AsyncWrite for Stream<R> {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            AsyncWrite::poll_write(Pin::new(&mut self.get_mut().0), cx, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().0), cx)
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().0), cx)
        }
    }
}

#[cfg(feature = "datagram")]
pub struct Datagram<U: Unreliable>(pub(crate) U);

#[cfg(feature = "datagram")]
impl<U: Unreliable> Datagram<U> {
    #[inline]
    pub async fn send(&self, packet: Associate) -> io::Result<()> {
        self.0.send(packet.to_bytes()?).await
    }

    #[inline]
    pub async fn recv(&self) -> io::Result<Associate> {
        let mut bytes = self.0.recv().await?;
        Associate::from_bytes(&mut bytes)
    }
}
