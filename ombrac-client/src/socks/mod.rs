use std::future::Future;
use std::io::{Error, Result};

use ombrac_protocol::request::{Address, Request};
use ombrac_protocol::Provider;

use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::mpsc::{self, Receiver};

use crate::{error, info};

pub struct SocksServer {
    inner: Receiver<(TcpStream, Request)>,
}

impl SocksServer {
    pub async fn with(addr: impl ToSocketAddrs) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;

        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            loop {
                let sender = sender.clone();
                match listener.accept().await {
                    Ok((stream, _address)) => {
                        tokio::spawn(async move {
                            let request = match Self::handle(stream).await {
                                Ok(value) => value,
                                Err(_error) => {
                                    error!("{}", _error);
                                    return;
                                }
                            };

                            if let Some(value) = request {
                                if sender.send(value).await.is_err() {
                                    return;
                                }
                            }
                        });
                    }

                    Err(_error) => {
                        error!("failed to accept: {:?}", _error);
                    }
                };
            }
        });

        Ok(Self { inner: receiver })
    }
}

impl Provider<(TcpStream, Request)> for SocksServer {
    async fn fetch(&mut self) -> Option<(TcpStream, Request)> {
        self.inner.recv().await
    }
}

mod socks5 {

    use socks::socks5::{Address as Socks5Address, Method as Socks5Method};
    use socks::socks5::{Request as Socks5Request, Response as Socks5Response};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    impl SocksServer {
        pub async fn handle(mut stream: TcpStream) -> Result<Option<(TcpStream, Request)>> {
            use socks::Streamable;

            let methods = <Vec<Socks5Method> as Streamable>::read(&mut stream).await?;

            // Authentication
            let method = <Self as socks5::Authentication>::select(methods).await?;
            <Socks5Method as Streamable>::write(&method, &mut stream).await?;

            // Process Authentication
            if !matches!(method, Socks5Method::NoAuthentication) {
                <Self as socks5::Authentication>::process(&mut stream).await?;
            }

            // Read Request
            let request = <Socks5Request as Streamable>::read(&mut stream).await?;

            info!("SOCKS5 {:?}", request);

            match request {
                Socks5Request::Connect(address) => {
                    Socks5Response::unspecified_success()
                        .write(&mut stream)
                        .await?;

                    let address = match address {
                        Socks5Address::IPv4(value) => Address::IPv4(value),
                        Socks5Address::IPv6(value) => Address::IPv6(value),
                        Socks5Address::Domain(domain, port) => Address::Domain(domain, port),
                    };

                    return Ok(Some((stream, Request::TcpConnect(address))));
                }

                #[cfg(feature = "udp")]
                Socks5Request::Associate(_address) => {
                    use tokio::net::UdpSocket;

                    let inbound = UdpSocket::bind("[::]:0").await?;
                    let address = Socks5Address::from_socket_address(inbound.local_addr()?);

                    let (request_sender, request_receiver) = mpsc::channel(1);
                    let (response_sender, response_receiver) = mpsc::channel(1);

                    Socks5Response::Success(address).write(&mut stream).await?;

                    tokio::spawn(async move {
                        udp::hanlde(inbound, request_sender, response_receiver).await
                    });

                    return Ok(Some((
                        stream,
                        Request::UdpAssociate(Some((response_sender, request_receiver))),
                    )));
                }

                _ => {
                    Socks5Response::CommandNotSupported
                        .write(&mut stream)
                        .await?;

                    return Err(Error::other("unsupported socks command"));
                }
            };
        }
    }

    impl Authentication for SocksServer {
        async fn select(_methods: Vec<Socks5Method>) -> Result<Socks5Method> {
            Ok(Socks5Method::NoAuthentication)
        }

        async fn process<T>(_stream: &mut T) -> Result<()>
        where
            T: AsyncReadExt + AsyncWriteExt + Unpin + Send,
        {
            Ok(())
        }
    }

    pub trait Authentication {
        fn select(methods: Vec<Socks5Method>) -> impl Future<Output = Result<Socks5Method>> + Send;
        fn process<S>(stream: &mut S) -> impl Future<Output = Result<()>> + Send
        where
            S: AsyncReadExt + AsyncWriteExt + Unpin + Send;
    }

    #[cfg(feature = "udp")]
    pub mod udp {
        use ombrac_protocol::request::udp::Datagram;
        use socks::socks5::UdpPacket as Socks5UdpPacket;
        use socks::{Streamable, ToBytes};
        use tokio::net::UdpSocket;
        use tokio::sync::mpsc::{Receiver, Sender};

        use super::*;

        pub async fn hanlde(
            inbound: UdpSocket,
            request_sender: Sender<Datagram>,
            mut response_receiver: Receiver<Datagram>,
        ) -> Result<()> {
            tokio::try_join!(
                handle_request(&inbound, &request_sender),
                handle_response(&inbound, &mut response_receiver)
            )?;

            Ok(())
        }

        async fn handle_request(inbound: &UdpSocket, sender: &Sender<Datagram>) -> Result<()> {
            let mut buffer = vec![0u8; 4096];

            loop {
                let (size, client_addr) = inbound.recv_from(&mut buffer).await?;

                inbound.connect(client_addr).await?;
                let packet = Socks5UdpPacket::read(&mut &buffer[..size]).await?;
                let address = match packet.address {
                    Socks5Address::IPv4(addr) => Address::IPv4(addr),
                    Socks5Address::IPv6(addr) => Address::IPv6(addr),
                    Socks5Address::Domain(domain, port) => Address::Domain(domain, port),
                };

                let datagram = Datagram::with(address, packet.data.len() as u16, packet.data);

                if sender.send(datagram).await.is_err() {
                    return Ok(());
                }
            }
        }

        async fn handle_response(
            inbound: &UdpSocket,
            receiver: &mut Receiver<Datagram>,
        ) -> Result<()> {
            while let Some(datagram) = receiver.recv().await {
                let address = match datagram.address {
                    Address::IPv4(addr) => Socks5Address::IPv4(addr),
                    Address::IPv6(addr) => Socks5Address::IPv6(addr),
                    Address::Domain(domain, port) => Socks5Address::Domain(domain, port),
                };

                inbound
                    .send(&Socks5UdpPacket::un_frag(address, datagram.data).to_bytes())
                    .await?;
            }

            Ok(())
        }
    }
}
