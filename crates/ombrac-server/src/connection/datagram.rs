use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use moka::future::Cache;
use tokio::net::UdpSocket;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, warn};

use ombrac::protocol::{Address, UdpPacket};
use ombrac_transport::Connection;

use ombrac::reassembly::UdpReassembler;

pub(crate) struct DatagramTunnel<C: Connection> {
    connection: Arc<C>,
    shutdown: CancellationToken,
    sessions: Cache<u64, DatagramSession>,
    dns_cache: Cache<Bytes, SocketAddr>,
    reassembler: Arc<UdpReassembler>,
    fragment_id_counter: Arc<AtomicU32>,
}

#[derive(Clone)]
pub(crate) struct DatagramSession {
    socket: Arc<UdpSocket>,
    abort_handle: AbortHandle,
    destination: Address,
    upstream_bytes: Arc<AtomicU64>,
    downstream_bytes: Arc<AtomicU64>,
    created_at: Instant,
}

impl<C: Connection> DatagramTunnel<C> {
    pub(crate) fn new(connection: Arc<C>, shutdown: CancellationToken) -> Self {
        Self {
            connection,
            shutdown,
            sessions: Cache::builder()
                .max_capacity(8192)
                .time_to_idle(Duration::from_secs(65))
                .eviction_listener(|session_id, session: DatagramSession, cause| {
                    session.abort_handle.abort();
                    info!(
                        dest = %session.destination,
                        session_id = *session_id,
                        upstream_bytes = session.upstream_bytes.load(Ordering::Relaxed),
                        downstream_bytes = session.downstream_bytes.load(Ordering::Relaxed),
                        duration_ms = session.created_at.elapsed().as_millis(),
                        reason = ?cause
                    );
                })
                .build(),
            dns_cache: Cache::builder()
                .time_to_idle(Duration::from_secs(300))
                .build(),
            reassembler: Arc::new(UdpReassembler::default()),
            fragment_id_counter: Arc::new(AtomicU32::new(0)),
        }
    }

    pub(crate) async fn accept_loop(self) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                result = self.connection.read_datagram() => {
                    let packet_bytes = match result {
                        Ok(bytes) => bytes,
                        Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                        Err(e) => return Err(e),
                    };
                    self.process_upstream_packet(packet_bytes).await?;
                }
            }
        }

        Ok(())
    }

    async fn process_upstream_packet(&self, packet_bytes: Bytes) -> io::Result<()> {
        let packet = match UdpPacket::decode(&packet_bytes) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to decode UDP packet from client: {e}");
                return Ok(()); // Skip malformed packet
            }
        };

        if let Some((session_id, address, data)) = self.reassembler.process(packet).await? {
            let data_len = data.len() as u64;
            let session = self.get_or_create_session(session_id, &address).await?;
            session
                .upstream_bytes
                .fetch_add(data_len, Ordering::Relaxed);
            self.forward_to_destination(session.socket, address, data);
        }
        Ok(())
    }

    /// Forwards a reassembled datagram to its final destination.
    fn forward_to_destination(&self, socket: Arc<UdpSocket>, address: Address, data: Bytes) {
        let dns_cache = self.dns_cache.clone();

        tokio::spawn(async move {
            match Self::resolve_address(&address, &dns_cache).await {
                Ok(dest_addr) => {
                    if let Err(e) = socket.send_to(&data, dest_addr).await {
                        warn!(%dest_addr, error = %e, "Failed to send UDP packet to destination");
                    }
                }
                Err(e) => {
                    warn!(%address, error = %e, "Failed to resolve destination address");
                }
            }
        });
    }

    async fn resolve_address(
        address: &Address,
        dns_cache: &Cache<Bytes, SocketAddr>,
    ) -> io::Result<SocketAddr> {
        match address {
            Address::SocketV4(addr) => Ok(SocketAddr::V4(*addr)),
            Address::SocketV6(addr) => Ok(SocketAddr::V6(*addr)),
            Address::Domain(domain, port) => {
                if let Some(addr) = dns_cache.get(domain).await {
                    return Ok(addr);
                }
                let domain_str = String::from_utf8_lossy(domain);
                let addr = tokio::net::lookup_host(format!("{}:{}", domain_str, port))
                    .await?
                    .next()
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("DNS resolution failed for {}", domain_str),
                        )
                    })?;
                dns_cache.insert(domain.clone(), addr).await;
                Ok(addr)
            }
        }
    }

    /// Retrieves an existing UDP socket for a session or creates a new one.
    async fn get_or_create_session(
        &self,
        session_id: u64,
        dest_addr: &Address,
    ) -> io::Result<DatagramSession> {
        if let Some(session) = self.sessions.get(&session_id).await {
            return Ok(session);
        }

        let bind_addr = if Self::resolve_address(dest_addr, &self.dns_cache)
            .await?
            .is_ipv6()
        {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        let new_socket = Arc::new(UdpSocket::bind(bind_addr).await?);
        let downstream_bytes = Arc::new(AtomicU64::new(0));

        let abort_handle =
            self.spawn_downstream_loop(session_id, new_socket.clone(), downstream_bytes.clone());

        let session = DatagramSession {
            socket: new_socket,
            abort_handle,
            upstream_bytes: Arc::new(AtomicU64::new(0)),
            downstream_bytes,
            created_at: Instant::now(),
            destination: dest_addr.clone(),
        };
        self.sessions.insert(session_id, session.clone()).await;

        Ok(session)
    }

    /// Spawns a dedicated task for handling downstream traffic for a single UDP session.
    fn spawn_downstream_loop(
        &self,
        session_id: u64,
        socket: Arc<UdpSocket>,
        downstream_bytes: Arc<AtomicU64>,
    ) -> AbortHandle {
        let connection = Arc::clone(&self.connection);
        let token = self.shutdown.child_token();
        let frag_counter = Arc::clone(&self.fragment_id_counter);

        let handle = tokio::spawn(
            async move {
                Self::run_downstream_loop(
                    connection,
                    session_id,
                    socket,
                    frag_counter,
                    token,
                    downstream_bytes,
                )
                .await;
            }
            .instrument(tracing::info_span!("downstream_loop", session_id)),
        );

        handle.abort_handle()
    }

    async fn run_downstream_loop(
        connection: Arc<C>,
        session_id: u64,
        socket: Arc<UdpSocket>,
        fragment_id_counter: Arc<AtomicU32>,
        token: CancellationToken,
        downstream_bytes: Arc<AtomicU64>,
    ) {
        let max_datagram_size = connection.max_datagram_size().unwrap_or(1350);
        let overhead = UdpPacket::fragmented_overhead();
        let max_payload_size = max_datagram_size.saturating_sub(overhead).max(1);
        let mut buf = vec![0u8; 65535];

        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                result = socket.recv_from(&mut buf) => {
                    let (len, from_addr) = match result {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(error = %e, "Error receiving from remote socket");
                            break;
                        }
                    };

                    let address = Address::from(from_addr);
                    let data = Bytes::copy_from_slice(&buf[..len]);
                    downstream_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);

                    if data.len() <= max_payload_size {
                        let packet = UdpPacket::Unfragmented { session_id, address, data };
                        Self::send_packet_to_client(&connection, packet).await;
                    } else {
                        let fragment_id = fragment_id_counter.fetch_add(1, Ordering::Relaxed);
                        let fragments = UdpPacket::split_packet(session_id, address, data, max_payload_size, fragment_id);
                        for fragment in fragments {
                            if !Self::send_packet_to_client(&connection, fragment).await {
                                break;
                            }
                        }
                    }
                    debug!(from = %from_addr, bytes = len, "UDP downstream");
                }
            }
        }
    }

    async fn send_packet_to_client(client: &Arc<C>, packet: UdpPacket) -> bool {
        match packet.encode() {
            Ok(encoded) => {
                if client.send_datagram(encoded).await.is_err() {
                    return false;
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to encode UDP packet for client");
            }
        }
        true
    }
}
