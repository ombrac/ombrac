use std::io;

use ombrac::io::Streamable;
use ombrac::request::{Address, Request};
use ombrac::Provider;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct Client<T> {
    transport: T,
}

impl<Transport, Stream> Client<Transport>
where
    Transport: Provider<Item = Stream>,
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(transport: Transport) -> Self {
        Self { transport }
    }

    pub async fn outbound(&mut self) -> io::Result<Stream> {
        match self.transport.fetch().await {
            Some(value) => Ok(value),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Not outbound connected",
            )),
        }
    }

    pub async fn tcp_connect(outbound: &mut Stream, address: Address) -> io::Result<()> {
        <Request as Streamable>::write(Request::TcpConnect(address), outbound).await?;

        Ok(())
    }
}
