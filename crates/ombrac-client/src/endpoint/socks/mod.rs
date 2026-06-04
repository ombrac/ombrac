//! In-tree SOCKS5 endpoint.
//!
//! A self-contained, no-authentication SOCKS5 server that bridges incoming
//! `CONNECT` (and, with the `datagram` feature, `UDP ASSOCIATE`) requests onto
//! the ombrac QUIC tunnel. The wire protocol lives in [`protocol`].

mod protocol;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use ombrac_macros::{error, info, warn};
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::Client as QuicClient;

use crate::client::Client;

use protocol::{Address, Reply, Request, VERSION, encode_reply};

#[cfg(feature = "datagram")]
use {
    crate::connection::UdpSession,
    bytes::Bytes,
    protocol::UdpPacket,
    std::net::IpAddr,
    tokio::net::UdpSocket,
};

/// SOCKS5 server bound to a [`Client`].
pub struct Server {
    client: Arc<Client<QuicClient, QuicConnection>>,
}

impl Server {
    pub fn new(client: Arc<Client<QuicClient, QuicConnection>>) -> Self {
        Self { client }
    }

    /// Accepts connections until `shutdown` resolves.
    pub async fn run(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                biased;

                _ = &mut shutdown => return Ok(()),

                result = listener.accept() => {
                    let (stream, peer) = match result {
                        Ok(pair) => pair,
                        Err(_err) => {
                            error!("socks: failed to accept connection: {_err}");
                            continue;
                        }
                    };

                    let _ = stream.set_nodelay(true);
                    let client = self.client.clone();
                    tokio::spawn(async move {
                        if let Err(_err) = handle_connection(client, stream, peer).await {
                            warn!("socks: connection {peer} error: {_err}");
                        }
                    });
                }
            }
        }
    }
}

/// Runs the full SOCKS5 exchange for a single accepted connection: method
/// negotiation, request parsing, then command dispatch.
async fn handle_connection(
    client: Arc<Client<QuicClient, QuicConnection>>,
    mut stream: TcpStream,
    peer: SocketAddr,
) -> io::Result<()> {
    negotiate_method(&mut stream).await?;

    let request = Request::read(&mut stream).await?;
    match request {
        Request::Connect(address) => handle_connect(&client, &mut stream, address, peer).await,
        #[cfg(feature = "datagram")]
        Request::Associate(_) => handle_associate(&client, &mut stream).await,
        #[cfg(not(feature = "datagram"))]
        Request::Associate(_) => {
            reply_failure(&mut stream, Reply::CommandNotSupported).await?;
            Ok(())
        }
        Request::Bind(_) => {
            warn!("socks: bind command not supported");
            reply_failure(&mut stream, Reply::CommandNotSupported).await?;
            Ok(())
        }
    }
}

/// Reads the client's method list and selects no-authentication.
///
/// ```text
///  +----+----------+----------+        +----+--------+
///  |VER | NMETHODS | METHODS  |  --->  |VER | METHOD |
///  +----+----------+----------+        +----+--------+
/// ```
async fn negotiate_method(stream: &mut TcpStream) -> io::Result<()> {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;

    if header[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported SOCKS version",
        ));
    }

    let n = header[1] as usize;
    let mut methods = [0u8; 255];
    stream.read_exact(&mut methods[..n]).await?;

    // We only support no-authentication; reject clients that don't offer it.
    if !methods[..n].contains(&protocol::method::NO_AUTHENTICATION) {
        stream
            .write_all(&[VERSION, protocol::method::NO_ACCEPTABLE_METHOD])
            .await?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "no acceptable authentication method",
        ));
    }

    stream
        .write_all(&[VERSION, protocol::method::NO_AUTHENTICATION])
        .await?;
    Ok(())
}

/// Sends an error reply with an unspecified bound address.
async fn reply_failure(stream: &mut TcpStream, reply: Reply) -> io::Result<()> {
    stream
        .write_all(&encode_reply(reply, &Address::UNSPECIFIED))
        .await
}

/// Handles `CONNECT`: opens a tunnel stream to `address`, replies, then relays
/// bytes bidirectionally.
async fn handle_connect(
    client: &Arc<Client<QuicClient, QuicConnection>>,
    stream: &mut TcpStream,
    address: Address,
    peer: SocketAddr,
) -> io::Result<()> {
    let dst = address.to_string();

    // Connect first so the reply code reflects the real outcome (RFC 1928).
    let mut upstream = match client.open_bidirectional(address.into()).await {
        Ok(upstream) => upstream,
        Err(err) => {
            error!(dst_addr = %dst, error = %err, "tcp connect failed");
            reply_failure(stream, Reply::from_connect_error(&err)).await?;
            return Err(err);
        }
    };

    stream
        .write_all(&encode_reply(Reply::Succeeded, &Address::UNSPECIFIED))
        .await?;

    match ombrac_transport::io::copy_bidirectional(stream, &mut upstream).await {
        Ok(stats) => {
            info!(
                src_addr = %peer,
                dst_addr = %dst,
                send = stats.a_to_b_bytes,
                recv = stats.b_to_a_bytes,
                "tcp connect"
            );
            Ok(())
        }
        Err((err, stats)) => {
            error!(
                src_addr = %peer,
                dst_addr = %dst,
                send = stats.a_to_b_bytes,
                recv = stats.b_to_a_bytes,
                error = %err,
                "tcp connect"
            );
            Err(err)
        }
    }
}

/// Handles `UDP ASSOCIATE`: binds a relay socket, advertises it to the client,
/// then shuttles datagrams between the client and the tunnel until the control
/// connection closes.
#[cfg(feature = "datagram")]
async fn handle_associate(
    client: &Arc<Client<QuicClient, QuicConnection>>,
    stream: &mut TcpStream,
) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    let local_ip = stream.local_addr()?.ip();

    let relay_socket = UdpSocket::bind(bind_addr_for(local_ip)).await?;
    let relay_addr = SocketAddr::new(local_ip, relay_socket.local_addr()?.port());
    info!(src_addr = %peer, relay_addr = %relay_addr, "socks: udp associate started");

    stream
        .write_all(&encode_reply(Reply::Succeeded, &Address::from(relay_addr)))
        .await?;

    let session = client.open_associate();
    let result = udp_relay_loop(stream, relay_socket, session).await;
    if let Err(ref err) = result
        && !matches!(
            err.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        )
    {
        error!(src_addr = %peer, error = %err, "socks: udp associate failed");
    }
    result
}

/// Picks an unspecified bind address matching the control connection's family.
#[cfg(feature = "datagram")]
fn bind_addr_for(ip: IpAddr) -> SocketAddr {
    let unspecified = match ip {
        IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
    };
    SocketAddr::new(unspecified, 0)
}

/// The bidirectional UDP relay loop.
///
/// - SOCKS client -> relay socket -> ombrac tunnel -> destination
/// - destination -> ombrac tunnel -> relay socket -> SOCKS client
///
/// The TCP control connection is polled concurrently; its closure ends the
/// association, as mandated by RFC 1928.
#[cfg(feature = "datagram")]
async fn udp_relay_loop(
    stream: &mut TcpStream,
    relay_socket: UdpSocket,
    mut session: UdpSession<QuicClient, QuicConnection>,
) -> io::Result<()> {
    let mut client_addr: Option<SocketAddr> = None;
    let mut buf = vec![0u8; u16::MAX as usize];

    loop {
        tokio::select! {
            biased;

            result = stream.read_u8() => {
                match result {
                    Ok(_) => {} // unexpected data on the control channel; ignore
                    Err(_) => {
                        info!("socks: control connection closed, ending udp session");
                        return Ok(());
                    }
                }
            }

            Some((data, from)) = session.recv_from() => {
                if let Some(dest) = client_addr {
                    let from = Address::try_from(from)?;
                    let packet = UdpPacket::encode_unfragmented(&from, &data);
                    relay_socket.send_to(&packet, dest).await?;
                } else {
                    warn!("socks: tunnel packet arrived before client, discarding");
                }
            }

            result = relay_socket.recv_from(&mut buf) => {
                let (len, src) = result?;
                if client_addr.is_none() {
                    client_addr = Some(src);
                    info!(client_addr = %src, "socks: first udp packet from client");
                }
                let packet = UdpPacket::from_buf(Bytes::copy_from_slice(&buf[..len]))?;
                // Fragmented datagrams are unsupported; drop per RFC 1928.
                if packet.frag != 0 {
                    warn!("socks: dropping fragmented udp datagram");
                    continue;
                }
                session.send_to(packet.data, packet.address.into()).await?;
            }
        }
    }
}
