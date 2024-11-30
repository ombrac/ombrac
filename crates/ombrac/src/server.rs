use std::future::Future;
use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::io::{IntoSplit, Streamable};
use crate::request::{Address, Request};

pub trait Server<Stream>: Sized + Send
where
    Stream: IntoSplit + AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    fn handler(mut stream: Stream) -> impl Future<Output = io::Result<()>> + Send {
        use crate::io::utils::copy_bidirectional;

        async move {
            let request = <Request as Streamable>::read(&mut stream).await?;

            match request {
                Request::TcpConnect(address) => {
                    let address = Self::resolve(address).await?;
                    let outbound = TcpStream::connect(address).await?;

                    copy_bidirectional(stream, outbound).await?
                }
            };

            Ok(())
        }
    }

    fn resolve(address: Address) -> impl Future<Output = io::Result<SocketAddr>> + Send {
        use tokio::net::lookup_host;

        async move {
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
    }

    fn listen(self) -> impl Future<Output = ()> + Send;
}
