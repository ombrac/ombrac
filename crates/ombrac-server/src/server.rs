use std::io;

use ombrac::io::Streamable;
use ombrac::request::{Address, Request};
use ombrac::Provider;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use ombrac_macros::error;

pub struct Server<T> {
    transport: T,
}

impl<Transport, Stream> Server<Transport>
where
    Transport: Provider<Item = Stream>,
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(transport: Transport) -> Self {
        Self { transport }
    }

    async fn handler(mut stream: Stream) -> io::Result<()> {
        let request = Request::read(&mut stream).await?;

        match request {
            Request::TcpConnect(addr) => Self::handle_tcp_connect(stream, addr).await?,
        };

        Ok(())
    }

    async fn handle_tcp_connect<A>(mut stream: Stream, addr: A) -> io::Result<Stream>
    where
        A: Into<Address>,
    {
        let addr = addr.into().to_socket_addr().await?;
        let mut outbound = TcpStream::connect(addr).await?;

        ombrac::io::util::copy_bidirectional(&mut stream, &mut outbound).await?;

        Ok(stream)
    }

    pub async fn listen(&self) -> io::Result<()> {
        while let Some(stream) = self.transport.fetch().await {
            tokio::spawn(async move {
                if let Err(_error) = Self::handler(stream).await {
                    error!("{}", _error);
                }
            });
        }

        Ok(())
    }
}
