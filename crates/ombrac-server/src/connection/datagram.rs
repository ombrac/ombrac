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
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac::protocol::{Address, UdpPacket};
use ombrac::reassembly::UdpReassembler;
use ombrac_macros::{info, warn};
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
                    reason = %"ok",
                );
            })
            .build()
    }

    fn create_dns_cache() -> Cache<Bytes, SocketAddr> {
        Cache::builder().time_to_idle(DNS_CACHE_TTL).build()
    }

    /// Main loop to accept incoming datagrams from the client connection.
    pub(crate) async fn accept_loop(self) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                result = self.connection.read_datagram() => {
                    match result {
                        Ok(packet_bytes) => {
                            if let Err(e) = self.handle_upstream_packet(packet_bytes).await {
                                warn!("failed to handle upstream packet: {}", e);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            continue
                        },
                        Err(e) => return Err(e),
                    };
                }
            }
        }
        Ok(())
    }

    /// Handles a single upstream packet from the client.
    /// This involves decoding, reassembly, session management, and forwarding to the destination.
    async fn handle_upstream_packet(&self, packet_bytes: Bytes) -> io::Result<()> {
        let packet = match UdpPacket::decode(&packet_bytes) {
            Ok(p) => p,
            Err(e) => {
                warn!("failed to decode udp packet from connection: {e}");
                return Ok(()); // Not an IO error, just a bad packet
            }
        };

        // Reassemble packet if fragmented
        if let Some((session_id, address, data)) = self.reassembler.process(packet).await? {
            let session = self.get_or_create_session(session_id, &address).await?;
            // This is a cheap, reference-counted clone, not a deep copy of the cache data.
            let dns_cache = self.dns_cache.clone();

            let future = async move {
                session
                    .upstream_bytes
                    .fetch_add(data.len() as u64, Ordering::Relaxed);

                match lookup_host(&dns_cache, &address).await {
                    Ok(dest_addr) => {
                        if let Err(err) = session.socket.send_to(&data, dest_addr).await {
                            warn!("failed to send udp packet to {address}: {err}");
                        }
                    }
                    Err(err) => {
                        warn!("failed to resolve DNS for {address}: {err}");
                    }
                }
            };

            #[cfg(not(feature = "tracing"))]
            tokio::spawn(future);
            #[cfg(feature = "tracing")]
            tokio::spawn(future.in_current_span());
        }
        Ok(())
    }

    /// Retrieves an existing session or creates a new one.
    /// A new session involves creating a UDP socket and spawning a downstream loop.
    async fn get_or_create_session(
        &self,
        session_id: u64,
        dest_addr: &Address,
    ) -> io::Result<Arc<DatagramSession>> {
        self.sessions
            .try_get_with(session_id, async {
                let bind_addr = match lookup_host(&self.dns_cache, dest_addr).await? {
                    SocketAddr::V4(_) => "0.0.0.0:0",
                    SocketAddr::V6(_) => "[::]:0",
                };

                let new_socket = Arc::new(UdpSocket::bind(bind_addr).await?);
                let upstream_bytes = Arc::new(AtomicU64::new(0));
                let downstream_bytes = Arc::new(AtomicU64::new(0));

                let abort_handle = self.spawn_downstream_loop(
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

    /// Spawns a new asynchronous task to handle the downstream traffic for a session.
    fn spawn_downstream_loop(
        &self,
        session_id: u64,
        socket: Arc<UdpSocket>,
        downstream_bytes: Arc<AtomicU64>,
    ) -> AbortHandle {
        let handler = DownstreamHandler {
            connection: Arc::clone(&self.connection),
            shutdown: self.shutdown.child_token(),
            fragment_id_counter: Arc::clone(&self.fragment_id_counter),
            session_id,
            socket,
            downstream_bytes,
        };

        #[cfg(feature = "tracing")]
        let abort = tokio::spawn(handler.accept_loop()).abort_handle();
        #[cfg(not(feature = "tracing"))]
        let abort = tokio::spawn(handler.accept_loop()).abort_handle();

        abort
    }
}

struct DownstreamHandler<C: Connection> {
    connection: Arc<C>,
    session_id: u64,
    socket: Arc<UdpSocket>,
    shutdown: CancellationToken,
    downstream_bytes: Arc<AtomicU64>,
    fragment_id_counter: Arc<AtomicU32>,
}

impl<C: Connection> DownstreamHandler<C> {
    /// Runs the downstream loop, receiving packets from the destination and sending them to the client connection.
    async fn accept_loop(self) {
        let overhead = UdpPacket::fragmented_overhead();
        let max_datagram_size = self
            .connection
            .max_datagram_size()
            .unwrap_or(DEFAULT_MAX_DATAGRAM_SIZE);
        let max_payload_size = max_datagram_size.saturating_sub(overhead).max(1);
        let mut buf = vec![0u8; MAX_UDP_PAYLOAD_SIZE];

        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                result = self.socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, from_addr)) => {
                            let address = Address::from(from_addr);
                            let data = Bytes::copy_from_slice(&buf[..len]);
                            self.downstream_bytes.fetch_add(len as u64, Ordering::Relaxed);

                            if let Err(_err) = self.process_and_send_datagram(address, data, max_payload_size).await {
                                warn!("failed to send packet to client, {_err} terminating loop.");
                                break;
                            }
                        },
                        Err(_err) => {
                            warn!("failed to receiving from remote socket {_err}");
                            break;
                        }
                    };
                }
            }
        }
    }

    /// Processes a packet, fragmenting it if necessary, and sends it to the client connection.
    async fn process_and_send_datagram(
        &self,
        address: Address,
        data: Bytes,
        max_payload_size: usize,
    ) -> io::Result<()> {
        if data.len() <= max_payload_size {
            let packet = UdpPacket::Unfragmented {
                session_id: self.session_id,
                address,
                data,
            };
            Self::send_datagram(&self.connection, packet).await?
        } else {
            let fragment_id = self.fragment_id_counter.fetch_add(1, Ordering::Relaxed);
            let fragments = UdpPacket::split_packet(
                self.session_id,
                address,
                data,
                max_payload_size,
                fragment_id,
            );
            for fragment in fragments {
                Self::send_datagram(&self.connection, fragment).await?
            }
        }

        Ok(())
    }

    async fn send_datagram(connection: &Arc<C>, packet: UdpPacket) -> io::Result<()> {
        let data = packet.encode()?;
        connection.send_datagram(data).await
    }
}

async fn lookup_host(
    dns_cache: &Cache<Bytes, SocketAddr>,
    address: &Address,
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
