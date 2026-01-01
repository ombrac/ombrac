use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use socks_lib::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use socks_lib::v5::server::Handler;
use socks_lib::v5::{Address as Socks5Address, Request, Response, Stream, UdpPacket};
use tokio::net::UdpSocket;

use ombrac_macros::{error, info, warn};
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::Client as QuicClient;

use crate::client::Client;
use crate::connection::BufferedStream;
#[cfg(feature = "datagram")]
use crate::connection::UdpSession;

pub struct CommandHandler {
    client: Arc<Client<QuicClient, QuicConnection>>,
}

impl CommandHandler {
    pub fn new(client: Arc<Client<QuicClient, QuicConnection>>) -> Self {
        Self { client }
    }

    /// Handles data forwarding after a successful connection.
    /// This method is called after the connection is established and response is sent.
    async fn handle_connect_forwarding(
        &self,
        stream: &mut Stream<impl AsyncRead + AsyncWrite + Unpin>,
        dest_stream: &mut BufferedStream<<QuicConnection as ombrac_transport::Connection>::Stream>,
        dst_addr: String,
    ) -> io::Result<()> {
        let src_addr = stream.peer_addr();
        match ombrac_transport::io::copy_bidirectional(stream, dest_stream).await {
            Ok(stats) => {
                info!(
                    src_addr = %src_addr,
                    dst_addr = %dst_addr,
                    send = stats.a_to_b_bytes,
                    recv = stats.b_to_a_bytes,
                    "connect"
                );
            }
            Err((err, stats)) => {
                error!(
                    src_addr = %src_addr,
                    dst_addr = %dst_addr,
                    send = stats.a_to_b_bytes,
                    recv = stats.b_to_a_bytes,
                    error = %err,
                    "connect"
                );
                return Err(err);
            }
        };

        Ok(())
    }

    /// Handles the SOCKS5 UDP ASSOCIATE command.
    #[cfg(feature = "datagram")]
    async fn handle_associate(
        &self,
        stream: &mut Stream<impl AsyncRead + AsyncWrite + Unpin + Send>,
    ) -> io::Result<()> {
        info!("handling udp associate from {}", stream.peer_addr());

        let udp_session = self.client.open_associate();

        let relay_socket = UdpSocket::bind("0.0.0.0:0").await?;
        let relay_addr = SocketAddr::new(
            stream.local_addr().ip(),
            relay_socket.local_addr().unwrap().port(),
        );
        info!("udp relay listening on {}", relay_addr);

        let response_addr = Socks5Address::from(relay_addr);
        stream
            .write_response(&Response::Success(&response_addr))
            .await?;

        self.udp_relay_loop(stream, relay_socket, udp_session).await
    }

    /// The main relay loop for a UDP association.
    ///
    /// This loop concurrently handles two data flows:
    /// - SOCKS Client -> Relay Socket -> ombrac Tunnel -> Destination
    /// - Destination -> ombrac Tunnel -> Relay Socket -> SOCKS Client
    #[cfg(feature = "datagram")]
    async fn udp_relay_loop(
        &self,
        stream: &mut Stream<impl AsyncRead + AsyncWrite + Unpin>,
        relay_socket: UdpSocket,
        mut udp_session: UdpSession<QuicClient, QuicConnection>,
    ) -> io::Result<()> {
        let mut client_udp_src: Option<SocketAddr> = None;
        let mut buf = vec![0u8; 65535]; // Max UDP packet size

        loop {
            tokio::select! {
                biased;
                result = stream.read_u8() => {
                    match result {
                        Ok(0) | Err(_) => {
                            info!("tcp control connection for udp associate closed, ending session");
                            return Ok(());
                        }
                        _ => {}
                    }
                }

                Some((data, from_addr)) = udp_session.recv_from() => {
                    if let Some(dest) = client_udp_src {
                        let socks_from_addr = util::ombrac_addr_to_socks(from_addr)?;
                        let udp_response = UdpPacket::un_frag(socks_from_addr, data);
                        relay_socket.send_to(&udp_response.to_bytes(), dest).await?;
                    } else {
                        warn!("received packet from tunnel before client, discarding");
                    }
                }

                result = relay_socket.recv_from(&mut buf) => {
                    let (len, src) = result?;
                    if client_udp_src.is_none() {
                        client_udp_src = Some(src);
                        info!("first udp packet received from client {}", src);
                    }
                    let mut bytes = Bytes::copy_from_slice(&buf[..len]);
                    let udp_request = UdpPacket::from_bytes(&mut bytes)?;
                    let payload = udp_request.data;
                    let dest_addr = util::socks_to_ombrac_addr(udp_request.address)?;

                    udp_session.send_to(payload, dest_addr).await?;
                }
            }
        }
    }
}

impl Handler for CommandHandler {
    async fn handle<S>(&self, stream: &mut Stream<S>, request: Request) -> io::Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        match request {
            Request::Connect(address) => {
                // Try to connect first, then send appropriate response
                // This ensures proper TCP state handling according to SOCKS5 protocol
                let dst_addr = util::socks_to_ombrac_addr(address.clone())?;
                let mut dest_stream = match self.client.open_bidirectional(dst_addr.clone()).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        // Connection failed - return error to let Handler trait handle the response
                        error!("connect to {} failed: {}", address, err);
                        return Err(err);
                    }
                };

                // Connection successful, send success response
                stream.write_response(&Response::Success(&address)).await?;

                // Now handle the data forwarding
                self.handle_connect_forwarding(stream, &mut dest_stream, address.to_string())
                    .await?
            }
            #[cfg(feature = "datagram")]
            Request::Associate(_) => {
                if let Err(err) = self.handle_associate(stream).await {
                    if err.kind() != io::ErrorKind::BrokenPipe
                        && err.kind() != io::ErrorKind::ConnectionReset
                    {
                        error!("associate from {} failed: {}", stream.peer_addr(), err);
                    }
                    return Err(err);
                }
            }
            _ => {
                warn!("bind command is not supported.");
                stream.write_response_unsupported().await?;
            }
        }

        Ok(())
    }
}

mod util {
    use ombrac::protocol::Address as OmbracAddress;
    use socks_lib::v5::Address as Socks5Address;
    use std::io;

    pub(super) fn socks_to_ombrac_addr(addr: Socks5Address) -> io::Result<OmbracAddress> {
        let result = match addr {
            Socks5Address::IPv4(value) => OmbracAddress::SocketV4(value),
            Socks5Address::IPv6(value) => OmbracAddress::SocketV6(value),
            Socks5Address::Domain(domain, port) => {
                OmbracAddress::Domain(domain.as_bytes().to_owned(), port)
            }
        };

        Ok(result)
    }

    pub(super) fn ombrac_addr_to_socks(addr: OmbracAddress) -> io::Result<Socks5Address> {
        let result = match addr {
            OmbracAddress::SocketV4(sa) => Socks5Address::IPv4(sa),
            OmbracAddress::SocketV6(sa) => Socks5Address::IPv6(sa),
            OmbracAddress::Domain(domain_bytes, port) => {
                Socks5Address::Domain(domain_bytes.try_into()?, port)
            }
        };

        Ok(result)
    }
}
