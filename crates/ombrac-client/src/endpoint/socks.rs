use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ombrac_macros::{debug, error, info, warn};
use ombrac_transport::Initiator;
use socks_lib::v5::{Address, Method, Request, Response, Stream};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
};

use crate::Client;

use crate::{Error, Result};

pub struct Server<T: Initiator>(TcpListener, Arc<Client<T>>);

impl<T: Initiator> Server<T> {
    pub async fn bind<A: Into<SocketAddr>>(addr: A, ombrac: Arc<Client<T>>) -> io::Result<Self> {
        let inner = TcpListener::bind(addr.into()).await?;
        Ok(Self(inner, ombrac))
    }

    pub async fn listen(&self) -> io::Result<()> {
        let ombrac = Arc::clone(&self.1);

        info!("SOCKS Server Listening on {}", self.0.local_addr()?);

        loop {
            match self.0.accept().await {
                Ok((inner, _addr)) => {
                    let ombrac = ombrac.clone();

                    tokio::spawn(async move {
                        let mut stream = Stream::with(inner);

                        let request = match Self::handle_socks_request(&mut stream).await {
                            Ok(request) => request,
                            Err(_error) => {
                                error!("Failed to accept socks: {}", _error);
                                return;
                            }
                        };

                        let result = match request {
                            Request::Connect(address) => {
                                match Self::handle_connect(ombrac, address.clone(), stream).await {
                                    Ok(_copy) => {
                                        info!(
                                            "Connect {:?}, Send: {}, Recv: {}",
                                            address, _copy.0, _copy.1
                                        );
                                        Ok(())
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            #[cfg(feature = "datagram")]
                            Request::Associate(address) => {
                                Self::handle_associate(ombrac, address, stream).await
                            }
                            _ => {
                                match stream.write_response(&Response::CommandNotSupported).await {
                                    Ok(_) => Ok(()),
                                    Err(e) => Err(Error::Io(e)),
                                }
                            }
                        };

                        if let Err(_error) = result {
                            error!("{}", _error);
                        }
                    });
                }

                Err(_error) => {
                    error!("Failed to accept: {}", _error);
                    continue;
                }
            }
        }
    }

    #[inline]
    async fn handle_socks_request(
        stream: &mut Stream<impl AsyncRead + AsyncWrite + Unpin>,
    ) -> Result<Request> {
        let _methods = stream.read_methods().await?;
        stream.write_auth_method(Method::NoAuthentication).await?;

        let request = stream.read_request().await?;

        Ok(request)
    }

    #[inline]
    async fn handle_connect(
        ombrac: Arc<Client<T>>,
        address: Address,
        mut stream: Stream<impl AsyncRead + AsyncWrite + Unpin>,
    ) -> Result<(u64, u64)> {
        use ombrac::address::Address as OmbracAddress;
        use ombrac::io::util::copy_bidirectional;
        use tokio::time::{sleep, timeout};

        const MAX_RETRIES: usize = 1;
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
        const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);

        stream
            .write_response(&Response::Success(Address::unspecified()))
            .await?;

        let addr = match address {
            Address::Domain(domain, port) => {
                OmbracAddress::Domain(domain.as_bytes().try_into()?, port)
            }
            Address::IPv4(addr) => OmbracAddress::IPv4(addr),
            Address::IPv6(addr) => OmbracAddress::IPv6(addr),
        };

        debug!("Connect {:?}", addr);

        let mut retry_count = 0;
        let mut retry_delay = INITIAL_RETRY_DELAY;
        let mut inbound = 0;
        let mut outbound = 0;

        loop {
            match timeout(CONNECT_TIMEOUT, ombrac.connect(addr.clone())).await {
                Ok(Ok(mut outbound_stream)) => {
                    (outbound, inbound) =
                        copy_bidirectional(&mut stream, &mut outbound_stream).await?;
                    break;
                }
                Ok(Err(_error)) => {
                    if retry_count >= MAX_RETRIES {
                        break;
                    }

                    warn!("Connect {:?} failed: {}", addr, _error.to_string());
                    retry_count += 1;

                    sleep(retry_delay).await;
                    retry_delay *= 2;
                }
                Err(_) => {
                    if retry_count >= MAX_RETRIES {
                        break;
                    }

                    warn!("Connect {:?} timed out", addr);
                    retry_count += 1;

                    sleep(retry_delay).await;
                    retry_delay *= 2;
                }
            }
        }

        Ok((outbound, inbound))
    }

    #[cfg(feature = "datagram")]
    #[inline]
    async fn handle_associate(
        ombrac: Arc<Client<T>>,
        _address: Address,
        mut stream: Stream<impl AsyncRead + AsyncWrite + Unpin>,
    ) -> Result<()> {
        use ombrac::address::Address as OmbracAddress;
        use socks_lib::v5::{Domain, UdpPacket};
        use tokio::net::UdpSocket;
        use tokio::time::timeout;

        const DEFAULT_BUF_SIZE: usize = 2 * 1024;
        const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        let addr = Address::from_socket_addr(socket.local_addr()?);

        stream.write_response(&Response::Success(&addr)).await?;
        drop(stream);

        let outbound = ombrac.associate().await?;

        let socket_1 = Arc::new(socket);
        let socket_2 = Arc::clone(&socket_1);
        let datagram_1 = Arc::new(outbound);
        let datagram_2 = Arc::clone(&datagram_1);

        let mut buf = [0u8; DEFAULT_BUF_SIZE];

        // Accpet first client
        let client_addr = {
            let (n, client_addr) = match timeout(IDLE_TIMEOUT, socket_1.recv_from(&mut buf)).await {
                Ok(Ok((n, addr))) => (n, addr),
                Ok(Err(e)) => return Err(Error::Io(e)),
                Err(_) => {
                    return Err(Error::Timeout("No initial packet received".to_string()));
                }
            };

            let packet = UdpPacket::from_bytes(&mut &buf[..n])?;

            let addr = match packet.address {
                Address::Domain(domain, port) => {
                    OmbracAddress::Domain(domain.to_bytes().try_into()?, port)
                }
                Address::IPv4(addr) => OmbracAddress::IPv4(addr),
                Address::IPv6(addr) => OmbracAddress::IPv6(addr),
            };

            datagram_1.send(packet.data, addr).await?;

            client_addr
        };

        let mut handle_1 = tokio::spawn(async move {
            loop {
                let (n, addr) = socket_1.recv_from(&mut buf).await?;

                // Only accept packets from the first client
                if addr != client_addr {
                    buf = [0u8; DEFAULT_BUF_SIZE];
                    continue;
                }

                let packet = UdpPacket::from_bytes(&mut &buf[..n])?;

                let addr = match packet.address {
                    Address::Domain(domain, port) => {
                        OmbracAddress::Domain(domain.to_bytes().try_into()?, port)
                    }
                    Address::IPv4(addr) => OmbracAddress::IPv4(addr),
                    Address::IPv6(addr) => OmbracAddress::IPv6(addr),
                };

                debug!(
                    "Associate send to {:?}, length: {}",
                    addr,
                    packet.data.len()
                );
                datagram_1.send(packet.data, addr).await?;
            }
        });

        let mut handle_2 = tokio::spawn(async move {
            loop {
                match timeout(IDLE_TIMEOUT, datagram_2.recv()).await {
                    Ok(Ok((data, addr))) => {
                        debug!("Associate recv from {:?}, length: {}", addr, data.len());

                        let addr = match addr {
                            OmbracAddress::Domain(domain, port) => {
                                Address::Domain(Domain::from_bytes(domain.to_bytes()), port)
                            }
                            OmbracAddress::IPv4(addr) => Address::IPv4(addr),
                            OmbracAddress::IPv6(addr) => Address::IPv6(addr),
                        };

                        let packet = UdpPacket::un_frag(addr, data);

                        socket_2.send_to(&packet.data, client_addr).await?;
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(_) => break, // Timeout
                }
            }

            Ok(())
        });

        let result = tokio::select! {
            result = &mut handle_1 => {
                handle_2.abort();
                result
            },
            result = &mut handle_2 => {
                handle_1.abort();
                result
            },
        };

        match result {
            Ok(inner_result) => inner_result,
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(Error::JoinError(e)),
        }
    }
}
