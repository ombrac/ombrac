use std::io;

use ombrac::request::{Address, Request};
use ombrac::Provider;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct Client<T> {
    transport: T,
}

impl<Transport, Stream> Client<Transport>
where
    Transport: Provider<Item = Stream>,
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(transport: Transport) -> Self {
        Self { transport }
    }

    async fn outbound(&self) -> io::Result<Stream> {
        match self.transport.fetch().await {
            Some(value) => Ok(value),
            None => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Connection has been lost",
            )),
        }
    }

    pub async fn tcp_connect<A>(&self, addr: A) -> io::Result<Stream>
    where
        A: Into<Address>,
    {
        use tokio::io::AsyncWriteExt;

        let request: Vec<u8> = Request::TcpConnect(addr.into()).into();
        let mut stream = self.outbound().await?;

        stream.write_all(&request).await?;

        Ok(stream)
    }
}
