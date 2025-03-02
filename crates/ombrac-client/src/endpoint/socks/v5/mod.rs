use std::future::Future;
use std::io;

use ombrac::prelude::*;
use socks_lib::socks5::{Address as Socks5Address, Method as Socks5Method};
use socks_lib::socks5::{Request as Socks5Request, Response as Socks5Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use super::{Request, Server};

impl Server {
    pub(super) async fn handler_v5(mut stream: TcpStream) -> io::Result<Request> {
        use socks_lib::Streamable;

        let methods = <Vec<Socks5Method> as Streamable>::read(&mut stream).await?;

        // Authentication
        let method = <Self as Authentication>::select(methods).await?;
        <Socks5Method as Streamable>::write(&method, &mut stream).await?;

        // Process Authentication
        if !matches!(method, Socks5Method::NoAuthentication) {
            <Self as Authentication>::process(&mut stream).await?;
        }

        // Read Request
        let request = <Socks5Request as Streamable>::read(&mut stream).await?;

        match request {
            Socks5Request::Connect(address) => {
                Socks5Response::unspecified_success()
                    .write(&mut stream)
                    .await?;

                let socks_address = match address {
                    Socks5Address::Domain(domain, port) => Address::Domain(domain, port),
                    Socks5Address::IPv4(addr) => Address::IPv4(addr),
                    Socks5Address::IPv6(addr) => Address::IPv6(addr),
                };

                return Ok(Request::TcpConnect(stream, socks_address));
            }

            #[cfg(feature = "datagram")]
            Socks5Request::Associate(_addr) => {
                let socket = UdpSocket::bind("0.0.0.0:0").await?;
                let addr = socket.local_addr().unwrap();
                let addr = Socks5Address::from_socket_address(addr);
                Socks5Response::Success(addr).write(&mut stream).await?;

                return Ok(Request::UdpAssociate(stream, socket));
            }

            _ => {
                Socks5Response::CommandNotSupported
                    .write(&mut stream)
                    .await?;

                return Err(io::Error::other("unsupported socks command"));
            }
        };
    }
}

pub trait Authentication {
    fn select(methods: Vec<Socks5Method>) -> impl Future<Output = io::Result<Socks5Method>> + Send;
    fn process<S>(stream: &mut S) -> impl Future<Output = io::Result<()>> + Send
    where
        S: AsyncReadExt + AsyncWriteExt + Unpin + Send;
}

impl Authentication for Server {
    async fn select(_methods: Vec<Socks5Method>) -> io::Result<Socks5Method> {
        Ok(Socks5Method::NoAuthentication)
    }

    async fn process<T>(_stream: &mut T) -> io::Result<()>
    where
        T: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    {
        Ok(())
    }
}
