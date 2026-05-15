//! Integration tests for the HTTP/HTTPS proxy endpoint.
//!
//! Exercises both proxy paths through a real QUIC tunnel:
//! - Plain HTTP (the client sends `GET http://origin/...`, the proxy forwards)
//! - HTTPS via CONNECT (the client sends `CONNECT origin:port HTTP/1.1`,
//!   the proxy upgrades and copies bytes)

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use ombrac::protocol::Secret;
use ombrac_client::client::Client as TunnelClient;
use ombrac_client::endpoint::http::Server as HttpServer;
use ombrac_server::connection::ConnectionAcceptor;
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::{Client as QuicClient, Config as QuicClientCfg};
use ombrac_transport::quic::server::{Config as QuicServerCfg, Server as QuicServer};

fn random_secret() -> Secret {
    use rand::Rng;
    let mut s = [0u8; 32];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut s);
    s
}

/// Build a running ombrac tunnel + HTTP proxy endpoint, returning the proxy
/// listen address (clients connect there with plain HTTP).
async fn build_http_proxy() -> (SocketAddr, broadcast::Sender<()>) {
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

    // 3. HTTP proxy endpoint listening on a free local port.
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let proxy_shutdown_rx = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let mut rx = proxy_shutdown_rx;
        let server = HttpServer::new(tunnel_client);
        let _ = server
            .accept_loop(proxy_listener, async move {
                let _ = rx.recv().await;
            })
            .await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    (proxy_addr, shutdown_tx)
}

/// A tiny HTTP/1.1 origin that returns `200 OK\r\n\r\nhello`. Doesn't try to
/// implement HTTP properly — just enough to satisfy the proxy's forwarded request.
async fn spawn_http_origin() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                // Read the request (until \r\n\r\n) — we don't care about contents.
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = match stream.read(&mut buf[total..]).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if total >= buf.len() {
                        return;
                    }
                }
                let body = "hello-from-origin";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    addr
}

/// Tiny TCP echo for CONNECT-tunneled traffic.
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

/// Reads from `stream` until EOF or `cap` bytes have been collected.
async fn read_until_eof(stream: &mut TcpStream, cap: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(cap.min(8192));
    let mut buf = vec![0u8; 4096];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() >= cap {
            break;
        }
    }
    Ok(out)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ntest::timeout(60000)]
async fn http_proxy_plain_get_returns_origin_body() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_http_proxy().await;
    let origin = spawn_http_origin().await;

    // Send absolute-URI GET (the format an HTTP proxy expects)
    let mut client = TcpStream::connect(proxy_addr).await?;
    let request = format!(
        "GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n",
    );
    client.write_all(request.as_bytes()).await?;
    client.flush().await?;

    let raw = read_until_eof(&mut client, 4096).await?;
    let text = String::from_utf8_lossy(&raw);

    assert!(text.starts_with("HTTP/1.1 200"), "expected 200, got: {text:?}");
    assert!(text.contains("hello-from-origin"), "body missing: {text:?}");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn http_proxy_connect_creates_tcp_tunnel() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_http_proxy().await;
    let echo = spawn_tcp_echo().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    let connect_req = format!(
        "CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n",
    );
    client.write_all(connect_req.as_bytes()).await?;

    // Read the proxy's 200 response (single line + headers, until \r\n\r\n).
    let mut header_buf = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        header_buf.extend_from_slice(&chunk[..n]);
        if header_buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if header_buf.len() > 4096 {
            panic!("CONNECT response did not terminate");
        }
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    assert!(
        header_str.starts_with("HTTP/1.1 200"),
        "expected CONNECT 200 response, got: {header_str:?}"
    );

    // After CONNECT, the connection is a raw byte tunnel to the echo server.
    client.write_all(b"hello-tunnel").await?;
    client.flush().await?;

    let mut echoed = [0u8; 12];
    client.read_exact(&mut echoed).await?;
    assert_eq!(&echoed, b"hello-tunnel");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn http_proxy_bad_request_no_host_in_uri() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_http_proxy().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    // Origin-form URI (no scheme/host) — proxies should reject because there's
    // no target to forward to.
    let request = "GET / HTTP/1.1\r\nHost: example.invalid\r\nConnection: close\r\n\r\n";
    client.write_all(request.as_bytes()).await?;

    let raw = read_until_eof(&mut client, 1024).await?;
    let text = String::from_utf8_lossy(&raw);
    // The proxy returns 400 (or another 4xx/5xx) — anything but 2xx is acceptable.
    assert!(
        !text.starts_with("HTTP/1.1 2"),
        "proxy should not return 2xx for missing-host request, got: {text:?}"
    );

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn http_proxy_unreachable_destination_returns_5xx() -> io::Result<()> {
    let (proxy_addr, shutdown) = build_http_proxy().await;

    let mut client = TcpStream::connect(proxy_addr).await?;
    // Port 1 on loopback: almost certainly nothing listening.
    let request = "GET http://127.0.0.1:1/ HTTP/1.1\r\nHost: 127.0.0.1:1\r\nConnection: close\r\n\r\n";
    client.write_all(request.as_bytes()).await?;

    let raw = read_until_eof(&mut client, 1024).await?;
    let text = String::from_utf8_lossy(&raw);
    // Should be 5xx (SERVICE_UNAVAILABLE per http.rs:99) when origin is unreachable.
    assert!(
        text.starts_with("HTTP/1.1 5"),
        "expected 5xx for unreachable origin, got: {text:?}"
    );

    let _ = shutdown.send(());
    Ok(())
}
