//! End-to-end benchmarks running a real QUIC tunnel between an in-process
//! ombrac client and ombrac server, both on loopback.
//!
//! Measures realistic numbers (closer to what users actually see) for:
//! - Stream-open latency (how fast a new tunnel stream is established)
//! - TCP throughput in 1 KB / 16 KB / 256 KB payloads over a single stream
//! - UDP datagram round-trip (with QUIC datagram extension)
//! - Concurrent stream handling

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast;

use ombrac::protocol::{Address, Secret};
use ombrac_client::client::Client as OmbracClient;
use ombrac_server::connection::ConnectionAcceptor;
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::{Client as QuicClient, Config as QuicClientCfg};
use ombrac_transport::quic::server::{Config as QuicServerCfg, Server as QuicServer};

// ── Test harness ─────────────────────────────────────────────────────────────

struct Harness {
    client: Arc<OmbracClient<QuicClient, QuicConnection>>,
    _shutdown_tx: broadcast::Sender<()>,
    _server_handle: tokio::task::JoinHandle<()>,
    tcp_echo_addr: SocketAddr,
    udp_echo_addr: SocketAddr,
}

async fn build_harness() -> Harness {
    // 1. Bind a real QUIC server socket on loopback.
    let server_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr: SocketAddr = server_socket.local_addr().unwrap();

    // Random secret per harness — fresh authentication state.
    let secret: Secret = {
        use rand::Rng;
        let mut s = [0u8; 32];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut s);
        s
    };

    let mut server_cfg = QuicServerCfg::default();
    server_cfg.enable_self_signed = true;
    server_cfg.alpn_protocols = vec![b"h3".to_vec()];

    let quic_server = QuicServer::new(server_socket, server_cfg).await.unwrap();

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    let server_handle = tokio::spawn(async move {
        let acceptor = ConnectionAcceptor::new(quic_server, secret);
        let _ = acceptor.accept_loop(shutdown_rx).await;
    });

    // 2. Build a QUIC client that skips verification (matches insecure mode).
    let mut client_cfg = QuicClientCfg::new(server_addr, "localhost".to_string());
    client_cfg.skip_server_verification = true;
    client_cfg.alpn_protocols = vec![b"h3".to_vec()];

    let quic_client = QuicClient::new(client_cfg).unwrap();
    let ombrac_client = OmbracClient::new(quic_client, secret, None).await.unwrap();

    // 3. Spin up local TCP echo and UDP echo servers (the "destination" the tunnel proxies to).
    let tcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_echo_addr = tcp_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match tcp_listener.accept().await {
                Ok((mut s, _)) => {
                    tokio::spawn(async move {
                        let (mut r, mut w) = s.split();
                        let _ = tokio::io::copy(&mut r, &mut w).await;
                    });
                }
                Err(_) => break,
            }
        }
    });

    let udp_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let udp_echo_addr = udp_socket.local_addr().unwrap();
    let udp_arc = Arc::new(udp_socket);
    {
        let udp_arc = udp_arc.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65535];
            loop {
                match udp_arc.recv_from(&mut buf).await {
                    Ok((n, peer)) => {
                        let _ = udp_arc.send_to(&buf[..n], peer).await;
                    }
                    Err(_) => break,
                }
            }
        });
    }

    Harness {
        client: Arc::new(ombrac_client),
        _shutdown_tx: shutdown_tx,
        _server_handle: server_handle,
        tcp_echo_addr,
        udp_echo_addr,
    }
}

fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

// ── Stream-open latency ──────────────────────────────────────────────────────

fn bench_stream_open(c: &mut Criterion) {
    let runtime = make_runtime();
    let harness = runtime.block_on(build_harness());
    let dest: Address = harness.tcp_echo_addr.to_string().as_str().try_into().unwrap();

    c.bench_function("e2e/stream_open", |b| {
        b.to_async(&runtime).iter(|| {
            let client = harness.client.clone();
            let dest = dest.clone();
            async move {
                let _stream = client.open_bidirectional(dest).await.unwrap();
                // Drop the stream — measure only open cost.
            }
        });
    });
}

// ── TCP throughput (single stream, round-trip echo) ──────────────────────────

fn bench_tcp_echo_throughput(c: &mut Criterion) {
    let runtime = make_runtime();
    let harness = runtime.block_on(build_harness());
    let dest: Address = harness.tcp_echo_addr.to_string().as_str().try_into().unwrap();

    let mut group = c.benchmark_group("e2e/tcp_echo");
    let sizes: &[usize] = &[1024, 16 * 1024, 256 * 1024];

    for &size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.to_async(&runtime).iter_batched(
                || {
                    let payload: Vec<u8> = (0..size).map(|i| (i & 0xff) as u8).collect();
                    payload
                },
                |payload| {
                    let client = harness.client.clone();
                    let dest = dest.clone();
                    async move {
                        let mut stream = client.open_bidirectional(dest).await.unwrap();
                        stream.write_all(&payload).await.unwrap();
                        stream.flush().await.unwrap();

                        let mut buf = vec![0u8; payload.len()];
                        stream.read_exact(&mut buf).await.unwrap();
                        assert_eq!(buf.len(), payload.len());
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ── UDP datagram round-trip ──────────────────────────────────────────────────

fn bench_udp_datagram_rtt(c: &mut Criterion) {
    let runtime = make_runtime();
    let harness = runtime.block_on(build_harness());

    let mut group = c.benchmark_group("e2e/udp_datagram_rtt");
    let sizes: &[usize] = &[64, 512, 1200];

    for &size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.to_async(&runtime).iter_batched(
                || Bytes::from(vec![0xab; size]),
                |payload| {
                    let client = harness.client.clone();
                    let dest_addr: Address =
                        harness.udp_echo_addr.to_string().as_str().try_into().unwrap();
                    async move {
                        let mut session = client.open_associate();
                        session.send_to(payload.clone(), dest_addr).await.unwrap();
                        let (resp, _from) = session.recv_from().await.unwrap();
                        assert_eq!(resp.len(), payload.len());
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ── Concurrent streams (open + tiny ping) ────────────────────────────────────

fn bench_concurrent_streams(c: &mut Criterion) {
    let runtime = make_runtime();
    let harness = runtime.block_on(build_harness());
    let dest: Address = harness.tcp_echo_addr.to_string().as_str().try_into().unwrap();

    let mut group = c.benchmark_group("e2e/concurrent_streams");
    for &count in &[4usize, 16, 64] {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.to_async(&runtime).iter(|| {
                let client = harness.client.clone();
                let dest = dest.clone();
                async move {
                    let mut handles = Vec::with_capacity(count);
                    for _ in 0..count {
                        let client = client.clone();
                        let dest = dest.clone();
                        handles.push(tokio::spawn(async move {
                            let mut stream = client.open_bidirectional(dest).await.unwrap();
                            stream.write_all(b"ping").await.unwrap();
                            stream.flush().await.unwrap();
                            let mut buf = [0u8; 4];
                            stream.read_exact(&mut buf).await.unwrap();
                            assert_eq!(&buf, b"ping");
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(std::time::Duration::from_secs(5));
    targets =
        bench_stream_open,
        bench_tcp_echo_throughput,
        bench_udp_datagram_rtt,
        bench_concurrent_streams,
);
criterion_main!(benches);
