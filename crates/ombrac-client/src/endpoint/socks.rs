use std::sync::Arc;
use std::time::Duration;

use ombrac::address::Address as OmbracAddress;
use ombrac_macros::{debug, info, warn};
use ombrac_transport::Initiator;
use socks_lib::io::{self, AsyncRead, AsyncWrite};
#[cfg(feature = "datagram")]
use socks_lib::net::UdpSocket;
#[cfg(feature = "datagram")]
use socks_lib::v5::UdpPacket;
use socks_lib::v5::server::Handler;
use socks_lib::v5::{Address as SocksAddress, Request, Stream};

use crate::Client;

pub struct CommandHandler<I: Initiator>(Arc<Client<I>>);

impl<I: Initiator> CommandHandler<I> {
    pub fn new(inner: Arc<Client<I>>) -> Self {
        Self(inner)
    }

    #[inline]
    async fn handle_connect(
        &self,
        address: SocksAddress,
        stream: &mut Stream<impl AsyncRead + AsyncWrite + Unpin>,
    ) -> io::Result<(u64, u64)> {
        use ombrac::io::util::copy_bidirectional;
        use tokio::time::{sleep, timeout};

        const MAX_ATTEMPTS: usize = 2;
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
        const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);

        let addr = util::socks_to_ombrac_addr(address)?;

        let mut last_error: Option<io::Error> = None;
        let mut retry_delay = INITIAL_RETRY_DELAY;

        for attempt in 1..=MAX_ATTEMPTS {
            if attempt > 1 {
                sleep(retry_delay).await;
                retry_delay *= 2;
            }

            warn!("Attempt {} to connect to {:?}", attempt, addr);

            match timeout(CONNECT_TIMEOUT, self.0.connect(addr.clone())).await {
                Ok(Ok(mut outbound_stream)) => {
                    return copy_bidirectional(stream, &mut outbound_stream).await;
                }

                Err(_) => {
                    let err =
                        io::Error::new(io::ErrorKind::TimedOut, "connection attempt timed out");
                    warn!("Connect to {:?} failed: {}", addr, err);
                    last_error = Some(err);
                }

                Ok(Err(e)) => {
                    warn!("Connect to {:?} failed: {}", addr, e);
                    last_error = Some(io::Error::other(e));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            io::Error::other("All connection attempts failed without a specific error")
        }))
    }

    #[cfg(feature = "datagram")]
    #[inline]
    async fn handle_associate(&self, socket: UdpSocket) -> io::Result<()> {
        use tokio::time::timeout;

        const DEFAULT_BUF_SIZE: usize = 2 * 1024;
        const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

        let outbound = self.0.associate().await.unwrap();

        let socket_1 = Arc::new(socket);
        let socket_2 = Arc::clone(&socket_1);
        let datagram_1 = Arc::new(outbound);
        let datagram_2 = Arc::clone(&datagram_1);

        let mut buf = [0u8; DEFAULT_BUF_SIZE];

        // Accpet first client
        let client_addr = {
            let (n, client_addr) = match timeout(IDLE_TIMEOUT, socket_1.recv_from(&mut buf)).await {
                Ok(Ok((n, addr))) => (n, addr),
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "No initial packet received",
                    ));
                }
            };

            let packet = UdpPacket::from_bytes(&mut &buf[..n])?;

            let addr = util::socks_to_ombrac_addr(SocksAddress::from(client_addr)).unwrap();

            datagram_1
                .send(packet.data, addr)
                .await
                .map_err(io::Error::other)?;

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
                let addr = OmbracAddress::from(addr);

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

                        let addr = util::ombrac_to_socks_addr(addr).unwrap();
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
            Ok(inner) => Ok(inner.map_err(io::Error::other)?),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(io::Error::other(e)),
        }
    }
}

impl<I: Initiator> Handler for CommandHandler<I> {
    async fn handle<T>(&self, stream: &mut Stream<T>, request: Request) -> io::Result<()>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        debug!("SOCKS Request: {:?}", request);

        match &request {
            Request::Connect(address) => {
                stream.write_response_unspecified().await?;

                match self.handle_connect(address.clone(), stream).await {
                    Ok(_copy) => {
                        info!(
                            "{} Connect {}, Send: {}, Recv: {}",
                            stream.peer_addr(),
                            address,
                            _copy.0,
                            _copy.1
                        );
                    }
                    Err(err) => return Err(err),
                }
            }
            #[cfg(feature = "datagram")]
            Request::Associate(_addr) => {
                use socks_lib::v5::Response;

                let socket = UdpSocket::bind("0.0.0.0:0").await?;
                let addr = SocksAddress::from(socket.local_addr()?);

                stream.write_response(&Response::Success(&addr)).await?;

                self.handle_associate(socket).await?;
            }
            _ => {
                stream.write_response_unsupported().await?;
            }
        }

        Ok(())
    }
}

mod util {
    use std::io;

    use ombrac::address::Address as OmbracAddress;
    use socks_lib::v5::Address as SocksAddress;

    #[inline]
    pub(super) fn socks_to_ombrac_addr(addr: SocksAddress) -> io::Result<OmbracAddress> {
        let result = match addr {
            SocksAddress::IPv4(value) => OmbracAddress::IPv4(value),
            SocksAddress::IPv6(value) => OmbracAddress::IPv6(value),
            SocksAddress::Domain(domain, port) => {
                OmbracAddress::Domain(domain.as_bytes().clone().try_into()?, port)
            }
        };

        Ok(result)
    }

    #[inline]
    pub(super) fn ombrac_to_socks_addr(addr: OmbracAddress) -> io::Result<SocksAddress> {
        let result = match addr {
            OmbracAddress::IPv4(value) => SocksAddress::IPv4(value),
            OmbracAddress::IPv6(value) => SocksAddress::IPv6(value),
            OmbracAddress::Domain(domain, port) => {
                SocksAddress::Domain(domain.to_bytes().try_into()?, port)
            }
        };

        Ok(result)
    }
}
