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
use tracing::{Instrument, info, warn};

use ombrac::protocol::{Address, UdpPacket};
use ombrac::reassembly::UdpReassembler;
use ombrac_transport::Connection;

const MAX_SESSIONS: u64 = 8192;
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(65);
const DNS_CACHE_TTL: Duration = Duration::from_secs(300);
const DEFAULT_MAX_DATAGRAM_SIZE: usize = 1350;
const MAX_UDP_PAYLOAD_SIZE: usize = 65535;

pub(crate) struct DatagramTunnel<C: Connection> {
    connection: Arc<C>,
    shutdown: CancellationToken,
    sessions: Cache<u64, Arc<DatagramSession>>,
    dns_cache: Cache<Bytes, SocketAddr>,
    reassembler: Arc<UdpReassembler>,
    fragment_id_counter: Arc<AtomicU32>,
}

pub(crate) struct DatagramSession {
    socket: Arc<UdpSocket>,
    upstream_bytes: Arc<AtomicU64>,
    downstream_bytes: Arc<AtomicU64>,
    abort_handle: AbortHandle,
    destination: Address,
    created_at: Instant,
}

impl<C: Connection> DatagramTunnel<C> {
    pub(crate) fn new(connection: Arc<C>, shutdown: CancellationToken) -> Self {
        Self {
            connection,
            shutdown,
            sessions: Self::create_session_cache(),
            dns_cache: Self::create_dns_cache(),
            reassembler: Arc::new(UdpReassembler::default()),
            fragment_id_counter: Arc::new(AtomicU32::new(0)),
        }
    }

    fn create_session_cache() -> Cache<u64, Arc<DatagramSession>> {
        Cache::builder()
            .max_capacity(MAX_SESSIONS)
            .time_to_idle(SESSION_IDLE_TIMEOUT)
            .eviction_listener(|session_id, session: Arc<DatagramSession>, _cause| {
                session.abort_handle.abort();
                #[cfg(feature = "tracing")]
                info!(
                    session_id = *session_id,
                    dest = %session.destination,
                    up = session.upstream_bytes.load(Ordering::Relaxed),
                    down = session.downstream_bytes.load(Ordering::Relaxed),
                    duration = session.created_at.elapsed().as_millis(),
                    reason = "ok",
                );
            })
            .build()
    }

    fn create_dns_cache() -> Cache<Bytes, SocketAddr> {
        Cache::builder().time_to_idle(DNS_CACHE_TTL).build()
    }

    pub(crate) async fn accept_loop(self) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                result = self.connection.read_datagram() => {
                    match result {
                        Ok(packet_bytes) => self.handle_upstream_packet(packet_bytes).await?,
                        Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                        Err(e) => return Err(e),
                    };
                }
            }
        }
        Ok(())
    }

    async fn handle_upstream_packet(&self, packet_bytes: Bytes) -> io::Result<()> {
        let packet = match UdpPacket::decode(&packet_bytes) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to decode UDP packet from client: {e}");
                return Ok(());
            }
        };

        if let Some((session_id, address, data)) = self.reassembler.process(packet).await? {
            let session = self.get_or_create_session(session_id, &address).await?;
            session
                .upstream_bytes
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            self.forward_upstream_packet(session.socket.clone(), address, data);
        }
        Ok(())
    }

    fn forward_upstream_packet(&self, socket: Arc<UdpSocket>, address: Address, data: Bytes) {
        let dns_cache = self.dns_cache.clone();
        tokio::spawn(
            async move {
                match lookup_host(&address, &dns_cache).await {
                    Ok(dest_addr) => {
                        if let Err(e) = socket.send_to(&data, dest_addr).await {
                            warn!(%dest_addr, error = %e, "Failed to send UDP packet to destination");
                        }
                    }
                    Err(e) => {
                        warn!(%address, error = %e, "Failed to resolve destination address");
                    }
                }
            }
            .in_current_span(),
        );
    }

    async fn get_or_create_session(
        &self,
        session_id: u64,
        dest_addr: &Address,
    ) -> io::Result<Arc<DatagramSession>> {
        self.sessions
            .try_get_with(session_id, async {
                let bind_addr = match lookup_host(dest_addr, &self.dns_cache).await? {
                    SocketAddr::V4(_) => "0.0.0.0:0",
                    SocketAddr::V6(_) => "[::]:0",
                };

                let new_socket = Arc::new(UdpSocket::bind(bind_addr).await?);
                let upstream_bytes = Arc::new(AtomicU64::new(0));
                let downstream_bytes = Arc::new(AtomicU64::new(0));

                let abort_handle = self.spawn_downstream_handler(
                    session_id,
                    new_socket.clone(),
                    downstream_bytes.clone(),
                );

                let session = DatagramSession {
                    socket: new_socket,
                    abort_handle,
                    upstream_bytes,
                    downstream_bytes,
                    created_at: Instant::now(),
                    destination: dest_addr.clone(),
                };

                Ok::<_, io::Error>(Arc::new(session))
            })
            .await
            .map_err(io::Error::other)
    }

    fn spawn_downstream_handler(
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
                downstream_loop(
                    connection,
                    session_id,
                    socket,
                    frag_counter,
                    token,
                    downstream_bytes,
                )
                .await;
            }
            .in_current_span(),
        );

        handle.abort_handle()
    }
}

async fn downstream_loop<C: Connection>(
    connection: Arc<C>,
    session_id: u64,
    socket: Arc<UdpSocket>,
    fragment_id_counter: Arc<AtomicU32>,
    token: CancellationToken,
    downstream_bytes: Arc<AtomicU64>,
) {
    let max_datagram_size = connection
        .max_datagram_size()
        .unwrap_or(DEFAULT_MAX_DATAGRAM_SIZE);
    let overhead = UdpPacket::fragmented_overhead();
    let max_payload_size = max_datagram_size.saturating_sub(overhead).max(1);
    let mut buf = vec![0u8; MAX_UDP_PAYLOAD_SIZE];

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
                    send_downstream_packet(&connection, packet).await;
                } else {
                    let fragment_id = fragment_id_counter.fetch_add(1, Ordering::Relaxed);
                    let fragments = UdpPacket::split_packet(session_id, address, data, max_payload_size, fragment_id);
                    for fragment in fragments {
                        if !send_downstream_packet(&connection, fragment).await {
                            break;
                        }
                    }
                };
            }
        }
    }
}

async fn send_downstream_packet<C: Connection>(client: &Arc<C>, packet: UdpPacket) -> bool {
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

async fn lookup_host(
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
