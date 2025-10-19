use std::net::SocketAddr;

use ombrac::reassembly::UdpReassembler;

use super::*;

#[derive(Clone)]
struct UdpSession {
    socket: Arc<UdpSocket>,
    abort_handle: AbortHandle,
    upstream_bytes: Arc<AtomicU64>,
    downstream_bytes: Arc<AtomicU64>,
    created_at: Instant,
    destination: Address,
}

/// Contains the shared state for a connection's UDP proxy.
pub(super) struct DatagramContext<C: Connection> {
    connection: Arc<C>,
    token: CancellationToken,
    session_sockets: Cache<u64, UdpSession>,
    dns_cache: Cache<Bytes, SocketAddr>,
    reassembler: Arc<UdpReassembler>,
    fragment_id_counter: Arc<AtomicU32>,
}

impl<C: Connection> DatagramContext<C> {
    pub(super) fn new(connection: Arc<C>, token: CancellationToken) -> Self {
        Self {
            connection,
            token,
            session_sockets: Cache::builder()
                .max_capacity(8192)
                .time_to_idle(Duration::from_secs(65))
                .eviction_listener(move |session_id, session: UdpSession, _cause| {
                    session.abort_handle.abort();
                    info!(
                            dest = %session.destination,
                            session_id = *session_id,
                            upstream_bytes = session.upstream_bytes.load(Ordering::Relaxed),
                            downstream_bytes = session.downstream_bytes.load(Ordering::Relaxed),
                            duration_ms = session.created_at.elapsed().as_millis(),
                            reason = ?_cause,
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

    /// Runs the main UDP proxy loop, handling upstream data from the client.
    pub(super) async fn run_associate_loop(self) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = self.token.cancelled() => {
                    return Ok(());
                }
                // Read a raw datagram from the client connection.
                result = self.connection.read_datagram() => {
                    let packet_bytes = match result {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            if e.kind() == io::ErrorKind::TimedOut {
                                continue;
                            }

                            return Err(e);
                        }
                    };
                    self.handle_upstream_packet(packet_bytes).await?;
                }
            }
        }
    }

    /// Processes a single raw packet received from the client.
    async fn handle_upstream_packet(&self, packet_bytes: Bytes) -> io::Result<()> {
        let packet = match UdpPacket::decode(&packet_bytes) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to decode UDP packet from client: {e}",);
                return Ok(()); // Skip malformed packet
            }
        };

        // Process the packet through the reassembler. It returns a full datagram when ready.
        if let Some((session_id, address, data)) = self.reassembler.process(packet).await? {
            let data_len = data.len() as u64;
            // Get or create the UDP socket for this session.
            let session = self
                .get_or_create_session_socket(session_id, &address)
                .await?;
            session
                .upstream_bytes
                .fetch_add(data_len, Ordering::Relaxed);
            self.forward_to_destination(session_id, session.socket.clone(), address, data);
        }
        Ok(())
    }

    /// Forwards a reassembled datagram to its final destination.
    fn forward_to_destination(
        &self,
        session_id: u64,
        socket: Arc<UdpSocket>,
        address: Address,
        data: Bytes,
    ) {
        let dns_cache = self.dns_cache.clone();

        tokio::spawn(async move {
            let dest_addr = match address {
                Address::SocketV4(addr) => SocketAddr::V4(addr),
                Address::SocketV6(addr) => SocketAddr::V6(addr),
                Address::Domain(ref domain, port) => {
                    if let Some(addr) = dns_cache.get(domain).await {
                        addr
                    } else {
                        let domain_str = String::from_utf8_lossy(domain);
                        match tokio::net::lookup_host(format!("{}:{}", domain_str, port)).await {
                            Ok(mut addrs) => {
                                if let Some(addr) = addrs.next() {
                                    dns_cache.insert(domain.clone(), addr).await;
                                    addr
                                } else {
                                    warn!(
                                        "[Session][{}] DNS resolution failed for {}",
                                        session_id, domain_str
                                    );
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "[Session][{}] DNS resolution error for {}: {}",
                                    session_id, domain_str, e
                                );
                                return;
                            }
                        }
                    }
                }
            };

            if let Err(e) = socket.send_to(&data, dest_addr).await {
                warn!("[Session] Failed to send packet to {}: {}", dest_addr, e);
            }
        });
    }

    /// Retrieves an existing UDP socket for a session or creates a new one.
    async fn get_or_create_session_socket(
        &self,
        session_id: u64,
        dest_addr: &Address,
    ) -> io::Result<UdpSession> {
        if let Some(session) = self.session_sockets.get(&session_id).await {
            return Ok(session);
        }

        let bind_addr = match dest_addr {
            Address::SocketV4(_) => "0.0.0.0:0",
            Address::SocketV6(_) => "[::]:0",
            Address::Domain(domain_bytes, port) => {
                if let Some(addr) = self.dns_cache.get(domain_bytes).await {
                    match addr {
                        SocketAddr::V4(_) => "0.0.0.0:0",
                        SocketAddr::V6(_) => "[::]:0",
                    }
                } else {
                    let domain = format!("{}:{}", String::from_utf8_lossy(domain_bytes), port);
                    match tokio::net::lookup_host(&domain).await?.next() {
                        Some(sa) if sa.is_ipv4() => "0.0.0.0:0",
                        Some(_) => "[::]:0",
                        None => {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!("Domain name {domain} could not be resolved"),
                            ));
                        }
                    }
                }
            }
        };

        let new_socket = Arc::new(UdpSocket::bind(bind_addr).await?);
        let downstream_bytes = Arc::new(AtomicU64::new(0));

        let abort_handle = self.spawn_downstream_task(
            session_id,
            Arc::clone(&new_socket),
            downstream_bytes.clone(),
        );
        let session = UdpSession {
            socket: Arc::clone(&new_socket),
            abort_handle,
            upstream_bytes: Arc::new(AtomicU64::new(0)),
            downstream_bytes,
            created_at: Instant::now(),
            destination: dest_addr.clone(),
        };
        self.session_sockets.insert(session_id, session).await;

        self.session_sockets
            .get(&session_id)
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Session not found"))
    }

    /// Spawns a dedicated task for handling downstream traffic for a single UDP session.
    fn spawn_downstream_task(
        &self,
        session_id: u64,
        socket: Arc<UdpSocket>,
        downstream_bytes: Arc<AtomicU64>,
    ) -> AbortHandle {
        let conn = Arc::clone(&self.connection);
        let token = self.token.child_token();
        let frag_counter = Arc::clone(&self.fragment_id_counter);

        let handle = tokio::spawn(async move {
            Self::run_downstream_task(
                conn,
                session_id,
                socket,
                frag_counter,
                token,
                downstream_bytes,
            )
            .await;
        });

        handle.abort_handle()
    }

    async fn run_downstream_task(
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
                            warn!("[Session][{}] Error receiving from remote socket: {}", session_id, e);
                            break;
                        }
                    };

                    let address = Address::from(from_addr);
                    let data = Bytes::copy_from_slice(&buf[..len]);
                    let data_len = data.len() as u64;
                    downstream_bytes.fetch_add(data_len, Ordering::Relaxed);

                    // This packet might need to be fragmented before sending back to client.
                    if data.len() <= max_payload_size {
                        let packet = UdpPacket::Unfragmented { session_id, address, data };
                        if let Ok(encoded) = packet.encode() {
                            if connection.send_datagram(encoded).await.is_err() {
                                break;
                            }
                        }
                    } else {
                        let fragment_id = fragment_id_counter.fetch_add(1, Ordering::Relaxed);
                        let fragments = UdpPacket::split_packet(session_id, address, data, max_payload_size, fragment_id);
                        for fragment in fragments {
                            if let Ok(encoded) = fragment.encode() {
                                if connection.send_datagram(encoded).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    tracing::debug!(from = %from_addr, bytes = data_len, "UDP downstream");
                }
            }
        }
    }
}
