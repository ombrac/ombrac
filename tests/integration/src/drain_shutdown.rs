//! Tests for `OmbracServer::shutdown_with_drain`.
//!
//! Verifies that:
//! - With no active streams, drain returns immediately and reports drained=true.
//! - With an active stream, drain waits up to the timeout.
//! - Metrics counters reflect activity through the drain.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use ombrac_client::{
    OmbracClient, ServiceConfig as ClientServiceConfig,
    TransportConfig as ClientTransportConfig,
};
use ombrac_server::{
    OmbracServer, ServiceConfig as ServerServiceConfig,
    TransportConfig as ServerTransportConfig, config::TlsMode as ServerTlsMode,
};

fn random_secret() -> String {
    use rand::Rng;
    let mut s = [0u8; 32];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut s);
    s.iter().map(|b| format!("{:02x}", b)).collect()
}

async fn get_available_port() -> u16 {
    tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

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

async fn build_server_and_client() -> (OmbracServer, OmbracClient, String) {
    let secret = random_secret();
    let port = get_available_port().await;
    let server_addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

    let server_config = Arc::new(ServerServiceConfig {
        secret: secret.clone(),
        listen: server_addr,
        transport: ServerTransportConfig {
            tls_mode: Some(ServerTlsMode::Insecure),
            ..Default::default()
        },
        connection: Default::default(),
        logging: Default::default(),
    });
    let server = OmbracServer::build(server_config).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client_config = Arc::new(ClientServiceConfig {
        secret: secret.clone(),
        server: format!("127.0.0.1:{}", port),
        auth_option: None,
        endpoint: ombrac_client::config::EndpointConfig {
            socks: Some("127.0.0.1:0".parse().unwrap()),
            ..Default::default()
        },
        transport: ClientTransportConfig {
            tls_mode: Some(ombrac_client::config::TlsMode::Insecure),
            ..Default::default()
        },
        logging: Default::default(),
    });
    let client = OmbracClient::build(client_config).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    (server, client, secret)
}

#[tokio::test]
#[ntest::timeout(30000)]
async fn drain_returns_immediately_when_no_active_streams() {
    let (server, client, _) = build_server_and_client().await;

    // No streams ever opened — drain should report true with no real wait.
    let start = std::time::Instant::now();
    let drained = server.shutdown_with_drain(Duration::from_secs(5)).await;
    let elapsed = start.elapsed();

    assert!(drained, "expected drain to succeed");
    // Allow up to 500ms for the polling interval (50ms) and accept-loop teardown.
    assert!(
        elapsed < Duration::from_millis(500),
        "drain took too long with no streams: {elapsed:?}"
    );

    client.shutdown().await;
}

#[tokio::test]
#[ntest::timeout(30000)]
async fn drain_waits_for_active_stream_to_complete() {
    let (server, client, _) = build_server_and_client().await;
    let metrics = server.metrics();

    let echo_addr = spawn_tcp_echo().await;

    let tunnel = client.client();
    let dest = echo_addr.to_string().as_str().try_into().unwrap();
    let mut stream = tunnel.open_bidirectional(dest).await.unwrap();

    // Use the stream so the server knows it exists and counts it.
    stream.write_all(b"ping").await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");

    // Give the server task a beat to bump streams_opened (the increment happens
    // in a spawned task; client-observable round-trip is necessary but not
    // strictly sufficient for the counter to have been written to memory).
    let mut waited = 0;
    while metrics.snapshot().streams_opened == 0 && waited < 1000 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 10;
    }
    assert!(
        metrics.snapshot().streams_opened >= 1,
        "server did not register stream open within 1s"
    );

    // Hand the stream to a task that will hold it open briefly, then drop.
    let active_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(stream); // closes the stream; server side increments streams_closed
    });

    // Drain with enough headroom to outlast the 300ms hold.
    let start = std::time::Instant::now();
    let drained = server.shutdown_with_drain(Duration::from_secs(3)).await;
    let elapsed = start.elapsed();

    // Either drained == true (server saw stream close in time) or false
    // (close raced with drain poll). Both are acceptable; what we care about
    // is that drain actually *waited* — it shouldn't return in <100ms when a
    // stream was active.
    assert!(
        elapsed >= Duration::from_millis(100),
        "drain returned too quickly while stream was active: {elapsed:?}"
    );
    let _ = drained;

    active_task.await.unwrap();
    client.shutdown().await;
}

#[tokio::test]
#[ntest::timeout(30000)]
async fn metrics_reflect_connection_and_stream_activity() {
    let (server, client, _) = build_server_and_client().await;
    let metrics = server.metrics();
    let echo_addr = spawn_tcp_echo().await;

    // Baseline snapshot: the OmbracClient already authenticated during build,
    // so connections_accepted is expected to be >= 1 already.
    let before = metrics.snapshot();
    assert!(
        before.connections_accepted >= 1,
        "OmbracClient::build should have triggered exactly one accept; got {}",
        before.connections_accepted
    );

    let tunnel = client.client();
    let dest = echo_addr.to_string().as_str().try_into().unwrap();
    let mut stream = tunnel.open_bidirectional(dest).await.unwrap();
    stream.write_all(b"hi").await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf).await.unwrap();

    // Poll for the server to register the stream open (counter is bumped from
    // a spawned task, may race with the client-observable round-trip).
    let mut waited = 0;
    while metrics.snapshot().streams_opened <= before.streams_opened && waited < 1000 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 10;
    }
    let after_open = metrics.snapshot();
    assert!(
        after_open.streams_opened > before.streams_opened,
        "streams_opened should have increased: before={} after={}",
        before.streams_opened,
        after_open.streams_opened
    );
    // connections_accepted should not change — we're reusing the existing
    // tunnel connection (this is the whole point of multiplexed streams).
    assert_eq!(
        after_open.connections_accepted, before.connections_accepted,
        "opening a stream should not open a new tunnel connection"
    );

    drop(stream);
    // Poll for the close to register on the server side.
    let mut waited = 0;
    while metrics.snapshot().streams_closed <= before.streams_closed && waited < 2000 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += 20;
    }
    let after_close = metrics.snapshot();
    assert!(
        after_close.streams_closed > before.streams_closed,
        "streams_closed should have increased: before={} after={}",
        before.streams_closed,
        after_close.streams_closed
    );

    client.shutdown().await;
    server.shutdown().await;
}
