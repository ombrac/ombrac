use std::io;

use ombrac::io::Streamable;
use ombrac::request::{Address, Request};
use ombrac::Provider;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use ombrac_macros::error;

pub struct Server<T> {
    secret: [u8; 32],
    transport: T,
}

impl<Transport, Stream> Server<Transport>
where
    Transport: Provider<Item = Stream>,
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(secret: [u8; 32], transport: Transport) -> Self {
        Self { secret, transport }
    }

    async fn handler(mut stream: Stream, secret: &[u8; 32]) -> io::Result<()> {
        let request = Request::read(&mut stream).await?;

        match request {
            Request::TcpConnect(client_auth, addr) => {
                if &client_auth != secret {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Authentication failed",
                    ));
                }
                Self::handle_tcp_connect(stream, addr).await?
            }
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
        let secret = self.secret.clone();

        while let Some(stream) = self.transport.fetch().await {
            tokio::spawn(async move {
                if let Err(e) = Self::handler(stream, &secret).await {
                    error!("{}", e);
                }
            });
        }

        Ok(())
    }
}
