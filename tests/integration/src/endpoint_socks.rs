//! Integration tests for the in-tree SOCKS5 proxy endpoint.
//!
//! Each test drives the raw SOCKS5 wire protocol over a real TCP socket,
//! through a real QUIC tunnel, exercising the method handshake, `CONNECT`
//! relaying, error replies, and (with the `datagram` feature) `UDP ASSOCIATE`.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::broadcast;

use ombrac::protocol::Secret;
use ombrac_client::client::Client as TunnelClient;
use ombrac_client::endpoint::socks::Server as SocksServer;
use ombrac_server::connection::ConnectionAcceptor;
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::{Client as QuicClient, Config as QuicClientCfg};
use ombrac_transport::quic::server::{Config as QuicServerCfg, Server as QuicServer};

const SOCKS5_VERSION: u8 = 0x05;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const CMD_CONNECT: u8 = 0x01;
const CMD_ASSOCIATE: u8 = 0x03;
const CMD_BIND: u8 = 0x02;

fn random_secret() -> Secret {
    use rand::Rng;
    let mut s = [0u8; 32];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut s);
    s
}

/// Build a running ombrac tunnel + SOCKS5 proxy endpoint, returning the proxy
/// listen address (clients speak SOCKS5 there).
async fn build_socks_proxy() -> (SocketAddr, broadcast::Sender<()>) {
    // 1. QUIC server on loopback (self-signed cert).
    let server_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr = server_udp.local_addr().unwrap();
    let secret = random_secret();

    let mut server_cfg = QuicServerCfg::default();
    server_cfg.enable_self_signed = true;
    server_cfg.alpn_protocols = vec![b"h3".to_vec()];
    let quic_server = QuicServer::new(server_udp, server_cfg).await.unwrap();

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    tokio::spawn(async move {
        let acceptor = ConnectionAcceptor::new(quic_server, secret);
        let _ = acceptor.accept_loop(shutdown_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2. QUIC client / ombrac tunnel client.
    let mut client_cfg = QuicClientCfg::new(server_addr, "localhost".to_string());
    client_cfg.skip_server_verification = true;
    client_cfg.alpn_protocols = vec![b"h3".to_vec()];
    let quic_client = QuicClient::new(client_cfg).unwrap();
    let tunnel_client: Arc<TunnelClient<QuicClient, QuicConnection>> =
        Arc::new(TunnelClient::new(quic_client, secret, None).await.unwrap());

    // 3. SOCKS5 proxy endpoint listening on a free local port.
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let proxy_shutdown_rx = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let mut rx = proxy_shutdown_rx;
        let server = SocksServer::new(tunnel_client);
        let _ = server
            .run(proxy_listener, async move {
                let _ = rx.recv().await;
            })
            .await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    (proxy_addr, shutdown_tx)
}

/// Tiny TCP echo server.
async fn spawn_tcp_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = stream.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// Performs the no-auth method handshake; returns once the server has selected
/// `NO_AUTHENTICATION` (0x00).
async fn negotiate_no_auth(stream: &mut TcpStream) -> io::Result<()> {
    // VER, NMETHODS, METHOD(no-auth)
    stream.write_all(&[SOCKS5_VERSION, 0x01, 0x00]).await?;
    let mut reply = [0u8; 2];
    stream.read_exact(&mut reply).await?;
    assert_eq!(reply[0], SOCKS5_VERSION, "bad version in method reply");
    assert_eq!(reply[1], 0x00, "server did not select no-auth");
    Ok(())
}

/// Sends a CONNECT request for an IPv4 `SocketAddr` and reads the reply header.
/// Returns the reply code (REP byte).
async fn send_connect_ipv4(stream: &mut TcpStream, dst: SocketAddr) -> io::Result<u8> {
    let SocketAddr::V4(v4) = dst else {
        panic!("expected ipv4");
    };
    let mut req = vec![SOCKS5_VERSION, CMD_CONNECT, 0x00, ATYP_IPV4];
    req.extend_from_slice(&v4.ip().octets());
    req.extend_from_slice(&v4.port().to_be_bytes());
    stream.write_all(&req).await?;

    read_reply_code(stream).await
}

/// Reads a full reply (`VER REP RSV ATYP ADDR PORT`) and returns the REP code.
async fn read_reply_code(stream: &mut TcpStream) -> io::Result<u8> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    assert_eq!(head[0], SOCKS5_VERSION, "bad version in reply");
    let rep = head[1];
    // Consume the bound address so the stream is positioned at the payload.
    let atyp = head[3];
    let addr_len = match atyp {
        ATYP_IPV4 => 4,
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            len[0] as usize
        }
        0x04 => 16,
        other => panic!("unexpected atyp in reply: {other}"),
    };
    let mut rest = vec![0u8; addr_len + 2];
    stream.read_exact(&mut rest).await?;
    Ok(rep)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ntest::timeout(60000)]
async fn socks_connect_relays_to_echo() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_socks_proxy().await;
    let echo = spawn_tcp_echo().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    negotiate_no_auth(&mut client).await?;
    let rep = send_connect_ipv4(&mut client, echo).await?;
    assert_eq!(rep, 0x00, "expected success reply");

    // Stream is now a raw tunnel to the echo server.
    client.write_all(b"hello-socks").await?;
    client.flush().await?;
    let mut buf = [0u8; 11];
    client.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"hello-socks");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn socks_connect_domain_relays_to_echo() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_socks_proxy().await;
    let echo = spawn_tcp_echo().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    negotiate_no_auth(&mut client).await?;

    // CONNECT via the domain address type. We use the "127.0.0.1" literal as
    // the domain so the tunnel server resolves it deterministically to the
    // IPv4 echo server (avoiding "localhost" dual-stack ambiguity).
    let host = b"127.0.0.1";
    let mut req = vec![SOCKS5_VERSION, CMD_CONNECT, 0x00, ATYP_DOMAIN, host.len() as u8];
    req.extend_from_slice(host);
    req.extend_from_slice(&echo.port().to_be_bytes());
    client.write_all(&req).await?;
    let rep = read_reply_code(&mut client).await?;
    assert_eq!(rep, 0x00, "expected success reply for domain connect");

    client.write_all(b"domain-ok").await?;
    client.flush().await?;
    let mut buf = [0u8; 9];
    client.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"domain-ok");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn socks_connect_refused_returns_error_reply() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_socks_proxy().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    negotiate_no_auth(&mut client).await?;

    // Port 1 on loopback: nothing should be listening.
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let rep = send_connect_ipv4(&mut client, dead).await?;
    assert_ne!(rep, 0x00, "expected a non-success reply for refused connect");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn socks_rejects_unsupported_method() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_socks_proxy().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    // Offer only GSSAPI (0x01), which the server does not support.
    client.write_all(&[SOCKS5_VERSION, 0x01, 0x01]).await?;
    let mut reply = [0u8; 2];
    client.read_exact(&mut reply).await?;
    assert_eq!(reply[0], SOCKS5_VERSION);
    assert_eq!(reply[1], 0xFF, "server should reject with NO_ACCEPTABLE_METHOD");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn socks_bind_is_unsupported() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_socks_proxy().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    negotiate_no_auth(&mut client).await?;

    let dst: SocketAddr = "127.0.0.1:9".parse().unwrap();
    let SocketAddr::V4(v4) = dst else { unreachable!() };
    let mut req = vec![SOCKS5_VERSION, CMD_BIND, 0x00, ATYP_IPV4];
    req.extend_from_slice(&v4.ip().octets());
    req.extend_from_slice(&v4.port().to_be_bytes());
    client.write_all(&req).await?;

    let rep = read_reply_code(&mut client).await?;
    assert_eq!(rep, 0x07, "BIND should yield COMMAND_NOT_SUPPORTED");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn socks_udp_associate_relays_datagram() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_socks_proxy().await;

    // A UDP echo server that bounces datagrams back to the sender.
    let echo = UdpSocket::bind("127.0.0.1:0").await?;
    let echo_addr = echo.local_addr()?;
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            let Ok((n, from)) = echo.recv_from(&mut buf).await else {
                return;
            };
            let _ = echo.send_to(&buf[..n], from).await;
        }
    });

    // Open the control connection and issue UDP ASSOCIATE.
    let mut control = TcpStream::connect(proxy_addr).await?;
    negotiate_no_auth(&mut control).await?;
    control
        .write_all(&[SOCKS5_VERSION, CMD_ASSOCIATE, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await?;

    // Reply carries the relay's bound address.
    let mut head = [0u8; 4];
    control.read_exact(&mut head).await?;
    assert_eq!(head[1], 0x00, "associate should succeed");
    assert_eq!(head[3], ATYP_IPV4, "relay address should be ipv4");
    let mut rest = [0u8; 6];
    control.read_exact(&mut rest).await?;
    let relay_port = u16::from_be_bytes([rest[4], rest[5]]);
    let relay_addr: SocketAddr = format!("127.0.0.1:{relay_port}").parse().unwrap();

    // Send a UDP request wrapping the payload + destination header.
    let client_udp = UdpSocket::bind("127.0.0.1:0").await?;
    let SocketAddr::V4(echo_v4) = echo_addr else {
        unreachable!()
    };
    let payload = b"udp-hello";
    let mut datagram = vec![0x00, 0x00, 0x00, ATYP_IPV4];
    datagram.extend_from_slice(&echo_v4.ip().octets());
    datagram.extend_from_slice(&echo_v4.port().to_be_bytes());
    datagram.extend_from_slice(payload);
    client_udp.send_to(&datagram, relay_addr).await?;

    // Read the relayed response (header + echoed payload).
    let mut buf = vec![0u8; 2048];
    let (n, _) = tokio::time::timeout(Duration::from_secs(10), client_udp.recv_from(&mut buf))
        .await
        .expect("timed out waiting for relayed udp response")?;
    // Skip RSV(2) + FRAG(1) + ATYP(1) + IPv4(4) + PORT(2) = 10 bytes of header.
    assert!(n > 10, "response too short: {n}");
    assert_eq!(&buf[10..n], payload, "relayed udp payload mismatch");

    // Keep the control connection alive until we've finished.
    drop(control);
    let _ = shutdown.send(());
    Ok(())
}
