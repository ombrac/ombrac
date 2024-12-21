use std::io;
use std::net::SocketAddr;

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
        let request = <Request as Streamable>::read(&mut stream).await?;

        match request {
            Request::TcpConnect(address) => {
                let address = Self::resolve(address).await?;
                let mut outbound = TcpStream::connect(address).await?;

                ombrac::io::util::copy_bidirectional(&mut stream, &mut outbound).await?
            }
        };

        Ok(())
    }

    async fn resolve(address: Address) -> io::Result<SocketAddr> {
        use tokio::net::lookup_host;

        match address {
            Address::Domain(domain, port) => lookup_host(format!("{}:{}", domain, port))
                .await?
                .next()
                .ok_or(io::Error::other(format!(
                    "could not resolve domain '{}:{}'",
                    domain, port
                ))),
            Address::IPv4(addr) => Ok(SocketAddr::V4(addr)),
            Address::IPv6(addr) => Ok(SocketAddr::V6(addr)),
        }
    }

    pub async fn listen(&mut self) -> io::Result<()> {
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
