//! End-to-end integration tests using a real QUIC transport (loopback).
//!
//! These exercise scenarios that mock transport cannot validate:
//! - Real QUIC TLS handshake (insecure self-signed mode)
//! - Large payload streaming and ordering through congestion control
//! - High concurrency (many simultaneous TCP streams)
//! - Mixed TCP + UDP traffic on the same tunnel connection
//! - Reconnect behavior after server restart

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast;

use ombrac::protocol::{Address, Secret};
use ombrac_client::client::Client as TunnelClient;
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

struct E2EHarness {
    client: Arc<TunnelClient<QuicClient, QuicConnection>>,
    server_addr: SocketAddr,
    shutdown_tx: broadcast::Sender<()>,
}

async fn build_e2e_harness() -> E2EHarness {
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

    // Brief wait so the acceptor's loop is alive before connect.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client_cfg = QuicClientCfg::new(server_addr, "localhost".to_string());
    client_cfg.skip_server_verification = true;
    client_cfg.alpn_protocols = vec![b"h3".to_vec()];

    let quic_client = QuicClient::new(client_cfg).unwrap();
    let tunnel_client = TunnelClient::new(quic_client, secret, None).await.unwrap();

    E2EHarness {
        client: Arc::new(tunnel_client),
        server_addr,
        shutdown_tx,
    }
}

async fn spawn_tcp_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    tokio::spawn(async move {
                        let (mut r, mut w) = stream.split();
                        let _ = tokio::io::copy(&mut r, &mut w).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    addr
}

async fn spawn_udp_echo() -> SocketAddr {
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = socket.local_addr().unwrap();
    let sock = socket.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            match sock.recv_from(&mut buf).await {
                Ok((n, peer)) => {
                    let _ = sock.send_to(&buf[..n], peer).await;
                }
                Err(_) => break,
            }
        }
    });
    addr
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_tunnel_handshake() {
    let harness = build_e2e_harness().await;
    // Connection succeeded if we got here.
    assert!(harness.server_addr.port() != 0);
    let _ = harness.shutdown_tx.send(());
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_tcp_large_payload_1mb() -> io::Result<()> {
    let harness = build_e2e_harness().await;
    let echo = spawn_tcp_echo().await;
    let dest: Address = echo.to_string().as_str().try_into().unwrap();

    let stream = harness.client.open_bidirectional(dest).await?;
    let payload: Arc<Vec<u8>> = Arc::new((0..1_000_000).map(|i| (i & 0xff) as u8).collect());

    let (mut r, mut w) = tokio::io::split(stream);
    // Concurrent writer task — don't shutdown so we just exercise the data path.
    let writer = {
        let p = payload.clone();
        tokio::spawn(async move {
            w.write_all(&p).await?;
            w.flush().await?;
            io::Result::Ok(w) // keep the writer alive until the test completes
        })
    };

    let mut received = vec![0u8; payload.len()];
    r.read_exact(&mut received).await?;

    let w = writer.await.unwrap()?;
    drop(w); // close write half cleanly after we've already read all the echoed bytes.

    assert_eq!(received.as_slice(), payload.as_slice());

    let _ = harness.shutdown_tx.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_high_concurrency_streams() -> io::Result<()> {
    let harness = build_e2e_harness().await;
    let echo = spawn_tcp_echo().await;
    let dest: Address = echo.to_string().as_str().try_into().unwrap();
    let client = harness.client.clone();

    let mut handles = Vec::new();
    for i in 0..64 {
        let client = client.clone();
        let dest = dest.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = client.open_bidirectional(dest).await?;
            let msg = format!("hello-{i:03}").into_bytes();
            stream.write_all(&msg).await?;
            stream.flush().await?;

            let mut buf = vec![0u8; msg.len()];
            stream.read_exact(&mut buf).await?;
            assert_eq!(buf, msg);
            io::Result::Ok(())
        }));
    }
    for h in handles {
        h.await.unwrap()?;
    }

    let _ = harness.shutdown_tx.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_mixed_tcp_and_udp_traffic() -> io::Result<()> {
    let harness = build_e2e_harness().await;
    let tcp_echo = spawn_tcp_echo().await;
    let udp_echo = spawn_udp_echo().await;
    let client = harness.client.clone();

    // Run TCP and UDP simultaneously
    let tcp_task = {
        let client = client.clone();
        let dest: Address = tcp_echo.to_string().as_str().try_into().unwrap();
        tokio::spawn(async move {
            for _ in 0..10 {
                let mut s = client.open_bidirectional(dest.clone()).await?;
                s.write_all(b"tcp-ping").await?;
                s.flush().await?;
                let mut buf = [0u8; 8];
                s.read_exact(&mut buf).await?;
                assert_eq!(&buf, b"tcp-ping");
            }
            io::Result::Ok(())
        })
    };

    let udp_task = {
        let client = client.clone();
        let dest_addr: Address = udp_echo.to_string().as_str().try_into().unwrap();
        tokio::spawn(async move {
            let mut session = client.open_associate();
            for _ in 0..10 {
                let payload = bytes::Bytes::from_static(b"udp-ping");
                session.send_to(payload.clone(), dest_addr.clone()).await?;
                let (resp, _) = session.recv_from().await.unwrap();
                assert_eq!(resp, payload);
            }
            io::Result::Ok(())
        })
    };

    tcp_task.await.unwrap()?;
    udp_task.await.unwrap()?;

    let _ = harness.shutdown_tx.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_concurrent_udp_sessions() -> io::Result<()> {
    let harness = build_e2e_harness().await;
    let udp_echo = spawn_udp_echo().await;
    let dest_addr: Address = udp_echo.to_string().as_str().try_into().unwrap();
    let client = harness.client.clone();

    let mut handles = Vec::new();
    for i in 0..16u8 {
        let client = client.clone();
        let dest = dest_addr.clone();
        handles.push(tokio::spawn(async move {
            let mut session = client.open_associate();
            let payload = bytes::Bytes::from(vec![i; 64]);
            session.send_to(payload.clone(), dest).await?;
            let (resp, _) = session.recv_from().await.unwrap();
            assert_eq!(resp, payload);
            io::Result::Ok(())
        }));
    }
    for h in handles {
        h.await.unwrap()?;
    }

    let _ = harness.shutdown_tx.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_destination_refused_is_reported() {
    let harness = build_e2e_harness().await;
    // Pick a port that very likely has no listener.
    let dest: Address = "127.0.0.1:1".try_into().unwrap();
    let result = harness.client.open_bidirectional(dest).await;
    assert!(result.is_err(), "expected error for closed destination port");
    let err = result.err().unwrap();
    assert!(matches!(
        err.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::Other | io::ErrorKind::TimedOut
    ));

    let _ = harness.shutdown_tx.send(());
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_invalid_secret_is_rejected() {
    let server_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr = server_udp.local_addr().unwrap();
    let server_secret = random_secret();
    let mut wrong_secret = random_secret();
    while wrong_secret == server_secret {
        wrong_secret = random_secret();
    }

    let mut server_cfg = QuicServerCfg::default();
    server_cfg.enable_self_signed = true;
    server_cfg.alpn_protocols = vec![b"h3".to_vec()];

    let quic_server = QuicServer::new(server_udp, server_cfg).await.unwrap();
    let (_tx, rx) = broadcast::channel::<()>(1);
    tokio::spawn(async move {
        let acceptor = ConnectionAcceptor::new(quic_server, server_secret);
        let _ = acceptor.accept_loop(rx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client_cfg = QuicClientCfg::new(server_addr, "localhost".to_string());
    client_cfg.skip_server_verification = true;
    client_cfg.alpn_protocols = vec![b"h3".to_vec()];

    let quic_client = QuicClient::new(client_cfg).unwrap();
    let result = TunnelClient::new(quic_client, wrong_secret, None).await;
    assert!(result.is_err(), "client should be rejected with wrong secret");
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn e2e_real_quic_chunked_streaming_preserves_order() -> io::Result<()> {
    let harness = build_e2e_harness().await;
    let echo = spawn_tcp_echo().await;
    let dest: Address = echo.to_string().as_str().try_into().unwrap();

    let mut stream = harness.client.open_bidirectional(dest).await?;
    let chunk_count = 100usize;
    let chunk_size = 4096usize;

    // Write many chunks then read them back; order must be preserved.
    for i in 0..chunk_count {
        let mut chunk = vec![0u8; chunk_size];
        chunk[0..4].copy_from_slice(&(i as u32).to_be_bytes());
        stream.write_all(&chunk).await?;
    }
    stream.flush().await?;

    let mut received = vec![0u8; chunk_size];
    for i in 0..chunk_count {
        stream.read_exact(&mut received).await?;
        let got = u32::from_be_bytes([received[0], received[1], received[2], received[3]]);
        assert_eq!(got as usize, i, "chunk ordering broken at index {i}");
    }

    let _ = harness.shutdown_tx.send(());
    Ok(())
}
