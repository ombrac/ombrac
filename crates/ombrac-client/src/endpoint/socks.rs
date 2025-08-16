use std::sync::Arc;

use ombrac::Secret;
use ombrac::client::Client;
use ombrac_macros::{debug, info};
use ombrac_transport::Initiator;
use socks_lib::io::{self, AsyncRead, AsyncWrite};
use socks_lib::v5::server::Handler;
use socks_lib::v5::{Address as SocksAddress, Request, Stream};

pub struct CommandHandler<I: Initiator> {
    ombrac_client: Arc<Client<I>>,
    secret: Secret,
}

impl<I: Initiator> CommandHandler<I> {
    pub fn new(inner: Arc<Client<I>>, secret: Secret) -> Self {
        Self {
            ombrac_client: inner,
            secret,
        }
    }

    async fn handle_connect(
        &self,
        address: SocksAddress,
        stream: &mut Stream<impl AsyncRead + AsyncWrite + Unpin>,
    ) -> io::Result<(u64, u64)> {
        let addr = util::socks_to_ombrac_addr(address)?;
        let mut outbound = self.ombrac_client.connect(addr, self.secret).await?;
        ombrac::io::util::copy_bidirectional(stream, &mut outbound).await
    }
}

#[cfg(feature = "datagram")]
mod datagram {
    use std::time::Duration;

    use ombrac::client::Datagram;
    use ombrac_transport::{Initiator, Unreliable};
    use socks_lib::v5::UdpPacket;
    use tokio::{net::UdpSocket, time::timeout};

    use super::*;

    const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
    const DEFAULT_BUFFER_SIZE: usize = 1500;

    impl<I: Initiator> CommandHandler<I> {
        pub async fn handle_associate(&self, socket: UdpSocket) -> io::Result<()> {
            let outbound = self.ombrac_client.associate().await?;
            let mut buf = vec![0u8; DEFAULT_BUFFER_SIZE];
            let (first_packet, source_addr) =
                match timeout(IDLE_TIMEOUT, socket.recv_from(&mut buf)).await {
                    Ok(Ok((_size, from))) => (UdpPacket::from_bytes(&mut (&buf[..]))?, from),
                    Ok(Err(e)) => return Err(e),
                    Err(_) => return Ok(()),
                };

            let packet = util::socks_to_ombrac_packet(first_packet, self.secret)?;
            outbound.send(packet).await?;
            socket.connect(source_addr).await?;

            let datagram = Arc::new(outbound);
            let udp_socket = Arc::new(socket);

            let client_to_target_task = tokio::spawn(proxy_ombrac_to_target(
                Arc::clone(&datagram),
                Arc::clone(&udp_socket),
                IDLE_TIMEOUT,
            ));

            let target_to_client_task = tokio::spawn(proxy_target_to_ombrac_server(
                datagram,
                udp_socket,
                self.secret,
                IDLE_TIMEOUT,
            ));

            let (client_res, target_res) =
                tokio::join!(client_to_target_task, target_to_client_task);

            client_res??;
            target_res??;

            Ok(())
        }
    }

    async fn proxy_ombrac_to_target<U>(
        datagram: Arc<Datagram<U>>,
        udp_socket: Arc<UdpSocket>,
        idle_timeout: Duration,
    ) -> io::Result<()>
    where
        U: Unreliable,
    {
        loop {
            let ombrac_packet = match timeout(idle_timeout, datagram.recv()).await {
                Ok(Ok(packet)) => packet,
                Ok(Err(e)) => return Err(e),
                Err(_) => break, // Timeout
            };

            let socks_packet = util::ombrac_to_socks_packet(ombrac_packet)?;
            udp_socket.send(&socks_packet.to_bytes()).await?;
        }
        Ok(())
    }

    async fn proxy_target_to_ombrac_server<U>(
        datagram: Arc<Datagram<U>>,
        udp_socket: Arc<UdpSocket>,
        session_secret: Secret,
        idle_timeout: Duration,
    ) -> io::Result<()>
    where
        U: Unreliable,
    {
        let mut buf = vec![0u8; DEFAULT_BUFFER_SIZE];
        loop {
            let n = match timeout(idle_timeout, udp_socket.recv(&mut buf)).await {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => return Err(e),
                Err(_) => break,
            };

            let socks_packet = UdpPacket::from_bytes(&mut (&buf[..n]))?;
            let packet = util::socks_to_ombrac_packet(socks_packet, session_secret)?;

            datagram.send(packet).await?;
        }
        Ok(())
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
                use tokio::net::UdpSocket;

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

    use ombrac::address::{Address as OmbracAddress, Domain as OmbracDoamin};
    #[cfg(feature = "datagram")]
    use ombrac::{Secret as OmbracSecret, associate::Associate as OmbracPacket};
    use socks_lib::v5::Address as Socks5Address;
    #[cfg(feature = "datagram")]
    use socks_lib::v5::UdpPacket as Socks5Packet;

    #[inline]
    #[cfg(feature = "datagram")]
    pub(super) fn socks_to_ombrac_packet(
        packet: Socks5Packet,
        secret: OmbracSecret,
    ) -> io::Result<OmbracPacket> {
        let addr = socks_to_ombrac_addr(packet.address)?;
        let data = packet.data;

        Ok(OmbracPacket::with(secret, addr, data))
    }

    #[inline]
    pub(super) fn socks_to_ombrac_addr(addr: Socks5Address) -> io::Result<OmbracAddress> {
        let result = match addr {
            Socks5Address::IPv4(value) => OmbracAddress::IPv4(value),
            Socks5Address::IPv6(value) => OmbracAddress::IPv6(value),
            Socks5Address::Domain(domain, port) => OmbracAddress::Domain(
                OmbracDoamin::from_bytes(domain.as_bytes().to_owned())?,
                port,
            ),
        };

        Ok(result)
    }

    #[inline]
    #[cfg(feature = "datagram")]
    pub(super) fn ombrac_to_socks_packet(packet: OmbracPacket) -> io::Result<Socks5Packet> {
        let addr = match packet.address {
            OmbracAddress::IPv4(value) => Socks5Address::IPv4(value),
            OmbracAddress::IPv6(value) => Socks5Address::IPv6(value),
            OmbracAddress::Domain(domain, port) => {
                Socks5Address::Domain(domain.to_bytes().try_into()?, port)
            }
        };
        let data = packet.data;

        Ok(Socks5Packet::un_frag(addr, data))
    }
}
