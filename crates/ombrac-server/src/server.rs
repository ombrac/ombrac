use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;

use ombrac::codec::{LengthDelimitedCodec, ServerHandshakeResponse, UpstreamMessage, length_codec};
use ombrac::protocol::{self, HandshakeError, PROTOCOLS_VERSION, Secret};
use ombrac_macros::{debug, error, info, warn};
use ombrac_transport::{Acceptor, Connection};

// Conditionally compiled datagram module. All UDP logic is encapsulated here.
#[cfg(feature = "datagram")]
use self::datagram::DatagraContext;

/// The main server struct, responsible for accepting incoming connections and spawning handlers.
pub struct Server<T: Acceptor> {
    acceptor: Arc<T>,
    secret: Secret,
}

impl<T: Acceptor> Server<T> {
    /// Creates a new server instance.
    pub fn new(acceptor: T, secret: Secret) -> Self {
        Self {
            acceptor: Arc::new(acceptor),
            secret,
        }
    }

    /// Runs the main server loop, accepting new connections.
    pub async fn accept_loop(&self, mut shutdown_rx: broadcast::Receiver<()>) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    return Ok(());
                }
                // Accept a new connection.
                accepted = self.acceptor.accept() => {
                    match accepted {
                        Ok(connection) => {
                            let secret = self.secret;
                            let peer_addr = connection.remote_address().unwrap_or_else(|_| "unknown".parse().unwrap());
                            info!("{} Connection established", peer_addr);

                            // Spawn a new task to handle the entire lifecycle of the client connection.
                            tokio::spawn(async move {
                                if let Err(e) = ConnectionHandler::handle(connection, secret, peer_addr).await {
                                    if e.kind() != io::ErrorKind::ConnectionReset && e.kind() != io::ErrorKind::BrokenPipe && e.kind() != io::ErrorKind::UnexpectedEof {
                                        error!("{} Connection handler failed: {}", peer_addr, e);
                                    } else {
                                        info!("{} Connection closed by peer", peer_addr);
                                    }
                                }
                            });
                        },
                        Err(_e) => {
                            error!("Failed to accept connection: {}", _e)
                        },
                    }
                },
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.acceptor.local_addr()
    }
}

/// Manages the lifecycle of a single client connection.
///
/// This struct is responsible for performing the initial handshake and then spawning
/// and managing the tasks for handling TCP and UDP traffic for the duration of the connection.
struct ConnectionHandler<C: Connection> {
    connection: Arc<C>,
    peer_addr: SocketAddr,
    cancellation_token: CancellationToken,
}

impl<C: Connection> ConnectionHandler<C> {
    /// The main entry point for handling a new client connection.
    pub async fn handle(connection: C, secret: Secret, peer_addr: SocketAddr) -> io::Result<()> {
        // Step 1: Perform the handshake to authenticate the client.
        let mut control_stream = connection.accept_bidirectional().await?;
        let mut framed_control = Framed::new(&mut control_stream, length_codec());

        match framed_control.next().await {
            Some(Ok(payload)) => {
                let hello_message: UpstreamMessage = protocol::decode(&payload)?;
                Self::validate_handshake(hello_message, secret, peer_addr, &mut framed_control)
                    .await?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Failed to read Hello message",
                ));
            }
        }

        // Step 2: Set up the handler and run the proxy tasks.
        let handler = Self {
            connection: Arc::new(connection),
            peer_addr,
            cancellation_token: CancellationToken::new(),
        };

        handler.run_proxy_tasks().await;
        Ok(())
    }

    /// Validates the client's Hello message and sends the appropriate response.
    async fn validate_handshake(
        message: UpstreamMessage,
        secret: Secret,
        _peer_addr: SocketAddr,
        framed: &mut Framed<&mut C::Stream, LengthDelimitedCodec>,
    ) -> io::Result<()> {
        if let UpstreamMessage::Hello(hello) = message {
            let response = if hello.version != PROTOCOLS_VERSION {
                warn!(
                    "{} Handshake failed: Unsupported protocol version",
                    _peer_addr
                );
                ServerHandshakeResponse::Err(HandshakeError::UnsupportedVersion)
            } else if hello.secret != secret {
                warn!("{} Handshake failed: Invalid secret", _peer_addr);
                ServerHandshakeResponse::Err(HandshakeError::InvalidSecret)
            } else {
                debug!("{} Handshake successful", _peer_addr);
                ServerHandshakeResponse::Ok
            };

            let response_bytes = protocol::encode(&response)?;
            framed.send(response_bytes).await?;

            if matches!(response, ServerHandshakeResponse::Err(_)) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Handshake failed",
                ));
            }
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected Hello message",
            ))
        }
    }

    /// Spawns and manages the long-running tasks for TCP and UDP proxying.
    ///
    /// This function waits for either the TCP or UDP handler to exit (due to an
    /// error or graceful shutdown) and then triggers a cancellation for all other
    /// tasks associated with this connection.
    async fn run_proxy_tasks(&self) {
        let tcp_handler = self.spawn_tcp_handler();

        #[cfg(feature = "datagram")]
        let udp_handler = self.spawn_udp_handler();

        // Wait for either handler to complete or fail.
        #[cfg(feature = "datagram")]
        let result = tokio::select! {
            res = tcp_handler => res,
            res = udp_handler => res,
        };

        // If datagram feature is disabled, only await the TCP handler.
        #[cfg(not(feature = "datagram"))]
        let result = tcp_handler.await;

        self.cancellation_token.cancel(); // Signal all related tasks to shut down.

        match result {
            Ok(Ok(_)) => {
                debug!("{} Client connection closed gracefully.", self.peer_addr);
            }
            Ok(Err(e)) => {
                if e.kind() != io::ErrorKind::ConnectionAborted {
                    warn!(
                        "{} Client connection closed with an error: {}",
                        self.peer_addr, e
                    );
                }
            }
            Err(_join_err) => {
                warn!(
                    "{} Client connection handler task failed: {}",
                    self.peer_addr, _join_err
                );
            }
        }
    }

    /// Spawns the task responsible for accepting and handling new TCP streams.
    fn spawn_tcp_handler(&self) -> JoinHandle<io::Result<()>> {
        let connection = Arc::clone(&self.connection);
        let peer_addr = self.peer_addr;
        let token = self.cancellation_token.child_token();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => return Ok(()),
                    result = connection.accept_bidirectional() => {
                        let stream = result?;

                        // Spawn a separate task for each TCP stream to avoid blocking the acceptor.
                        tokio::spawn(async move {
                            if let Err(_e) = Self::handle_tcp_stream(stream, peer_addr).await {
                                warn!("{} Stream handler error: {}", peer_addr, _e);
                            }
                        });
                    }
                }
            }
        })
    }

    /// Handles a single TCP stream, proxying data to the requested destination.
    async fn handle_tcp_stream(mut stream: C::Stream, _peer_addr: SocketAddr) -> io::Result<()> {
        let mut framed = Framed::new(&mut stream, length_codec());

        // Read the destination address from the client.
        let original_dest = match framed.next().await {
            Some(Ok(payload)) => match protocol::decode(&payload)? {
                UpstreamMessage::Connect(connect) => connect.address,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Expected Connect message",
                    ));
                }
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Failed to read Connect message on new stream",
                ));
            }
        };

        let mut dest_stream = TcpStream::connect(original_dest.to_socket_addr().await?).await?;

        // Forward any data that was already buffered in the framing codec.
        let parts = framed.into_parts();
        let mut stream = parts.io;
        if !parts.read_buf.is_empty() {
            dest_stream.write_all(&parts.read_buf).await?;
        }

        // Copy data in both directions until one side closes.
        match ombrac_transport::io::copy_bidirectional(&mut stream, &mut dest_stream).await {
            Ok(_stats) => {
                #[cfg(feature = "tracing")]
                tracing::info!(
                    src_addr = _peer_addr.to_string(),
                    send = _stats.a_to_b_bytes,
                    recv = _stats.b_to_a_bytes,
                    status = "ok",
                    "Connect"
                );
            }
            Err((err, _stats)) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    src_addr = _peer_addr.to_string(),
                    send = _stats.a_to_b_bytes,
                    recv = _stats.b_to_a_bytes,
                    status = "err",
                    error = %err,
                    "Connect"
                );
                return Err(err);
            }
        }

        Ok(())
    }

    /// Spawns the task responsible for handling all UDP traffic.
    #[cfg(feature = "datagram")]
    fn spawn_udp_handler(&self) -> JoinHandle<io::Result<()>> {
        let context = DatagraContext::new(
            Arc::clone(&self.connection),
            self.peer_addr,
            self.cancellation_token.child_token(),
        );
        tokio::spawn(async move { context.run_associate_loop().await })
    }
}

#[cfg(feature = "datagram")]
mod datagram {
    use std::io;
    use std::net::SocketAddr;
    use std::sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    };
    use std::time::Duration;

    use bytes::Bytes;
    use moka::future::Cache;
    use ombrac_macros::{debug, info, warn};
    use tokio::net::UdpSocket;
    use tokio::task::AbortHandle;
    use tokio_util::sync::CancellationToken;

    use ombrac::protocol::{Address, UdpPacket};
    use ombrac::reassembly::UdpReassembler;
    use ombrac_transport::Connection;

    /// Contains the shared state for a connection's UDP proxy.
    pub(super) struct DatagraContext<C: Connection> {
        connection: Arc<C>,
        peer_addr: SocketAddr,
        token: CancellationToken,
        session_sockets: Cache<u64, (Arc<UdpSocket>, AbortHandle)>,
        reassembler: Arc<UdpReassembler>,
        fragment_id_counter: Arc<AtomicU16>,
    }

    impl<C: Connection> DatagraContext<C> {
        pub(super) fn new(
            connection: Arc<C>,
            peer_addr: SocketAddr,
            token: CancellationToken,
        ) -> Self {
            Self {
                connection,
                peer_addr,
                token,
                session_sockets: Cache::builder()
                    .time_to_idle(Duration::from_secs(120))
                    .eviction_listener(|_key, val: (Arc<UdpSocket>, AbortHandle), _cause| {
                        val.1.abort();
                        debug!("Session UDP socket evicted due to: {:?}", _cause);
                    })
                    .build(),
                reassembler: Arc::new(UdpReassembler::default()),
                fragment_id_counter: Arc::new(AtomicU16::new(0)),
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
                                    debug!("{} Idle timeout reading datagram from client. Continuing.", self.peer_addr);
                                    continue;
                                }

                                warn!("{} Unrecoverable error reading datagram from client: {}. Closing UDP handler.", self.peer_addr, e);
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
                    warn!(
                        "{} Failed to decode UDP packet from client: {}",
                        self.peer_addr, e
                    );
                    return Ok(()); // Skip malformed packet
                }
            };

            // Process the packet through the reassembler. It returns a full datagram when ready.
            if let Some((session_id, address, data)) = self.reassembler.process(packet).await? {
                // Get or create the UDP socket for this session.
                let socket = self
                    .get_or_create_session_socket(session_id, address.clone())
                    .await?;
                self.forward_to_destination(session_id, socket, address, data)
                    .await;
            }
            Ok(())
        }

        /// Forwards a reassembled datagram to its final destination.
        async fn forward_to_destination(
            &self,
            session_id: u64,
            socket: Arc<UdpSocket>,
            address: Address,
            data: Bytes,
        ) {
            let dest_addr_str = address.to_string();
            let dest_addr = match tokio::net::lookup_host(&dest_addr_str).await {
                Ok(mut addrs) => addrs.next().unwrap(),
                Err(_) => {
                    warn!(
                        "{} [Session][{}] DNS resolution failed for {}",
                        self.peer_addr, session_id, dest_addr_str
                    );
                    return;
                }
            };

            debug!(
                "{} [Session][{}] Sending {} {} bytes",
                self.peer_addr,
                session_id,
                dest_addr,
                data.len()
            );
            if let Err(e) = socket.send_to(&data, dest_addr).await {
                warn!(
                    "{} [Session] Failed to send packet to {}: {}",
                    self.peer_addr, dest_addr, e
                );
            }
        }

        /// Retrieves an existing UDP socket for a session or creates a new one.
        async fn get_or_create_session_socket(
            &self,
            session_id: u64,
            dest_addr: Address,
        ) -> io::Result<Arc<UdpSocket>> {
            if let Some((socket, _)) = self.session_sockets.get(&session_id).await {
                return Ok(socket);
            }

            let bind_addr = match dest_addr {
                Address::SocketV4(_) => "0.0.0.0:0",
                Address::SocketV6(_) => "[::]:0",
                Address::Domain(_, _) => {
                    match tokio::net::lookup_host(dest_addr.to_string()).await?.next() {
                        Some(sa) if sa.is_ipv4() => "0.0.0.0:0",
                        Some(_) => "[::]:0",
                        None => {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                "Domain name could not be resolved",
                            ));
                        }
                    }
                }
            };

            // Create a new UDP socket bound to a random port.
            let new_socket = Arc::new(UdpSocket::bind(bind_addr).await?);

            info!(
                "{} [Session][{}] New session for {}, listening on {}",
                self.peer_addr,
                session_id,
                dest_addr,
                new_socket.local_addr()?
            );

            // Spawn a task to handle downstream traffic (from destination back to client).
            let abort_handle =
                self.spawn_downstream_task(session_id, Arc::clone(&new_socket), dest_addr);
            self.session_sockets
                .insert(session_id, (Arc::clone(&new_socket), abort_handle))
                .await;

            Ok(new_socket)
        }

        /// Spawns a dedicated task for handling downstream traffic for a single UDP session.
        fn spawn_downstream_task(
            &self,
            session_id: u64,
            socket: Arc<UdpSocket>,
            original_dest: Address,
        ) -> AbortHandle {
            let conn = Arc::clone(&self.connection);
            let token = self.token.child_token();
            let frag_counter = Arc::clone(&self.fragment_id_counter);
            let peer_addr = self.peer_addr;

            let handle = tokio::spawn(async move {
                Self::run_downstream_task(
                    conn,
                    peer_addr,
                    session_id,
                    socket,
                    frag_counter,
                    token,
                    original_dest,
                )
                .await;
            });

            handle.abort_handle()
        }

        /// Task that reads from a remote UDP socket and forwards data back to the client.
        async fn run_downstream_task(
            connection: Arc<C>,
            peer_addr: SocketAddr,
            session_id: u64,
            socket: Arc<UdpSocket>,
            fragment_id_counter: Arc<AtomicU16>,
            token: CancellationToken,
            original_dest: Address,
        ) {
            let max_datagram_size = connection.max_datagram_size().unwrap_or(1350);
            let overhead = UdpPacket::fragmented_overhead();
            let max_payload_size = max_datagram_size.saturating_sub(overhead).max(1);
            let mut buf = vec![0u8; 65535];

            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    result = socket.recv_from(&mut buf) => {
                        let (len, _from_addr) = match result {
                            Ok(r) => r,
                            Err(e) => {
                                warn!("{} [Session][{}] Error receiving from remote socket: {}", peer_addr, session_id, e);
                                break;
                            }
                        };

                        let address = original_dest.clone();
                        let data = Bytes::copy_from_slice(&buf[..len]);

                        debug!(
                            "{} [Session][{}] Receive {} {} bytes",
                            peer_addr, session_id, address, len
                        );

                        // This packet might need to be fragmented before sending back to client.
                        if data.len() <= max_payload_size {
                            let packet = UdpPacket::Unfragmented { session_id, address, data };
                            if let Ok(encoded) = packet.encode()
                                && connection.send_datagram(encoded).await.is_err() { break; }
                        } else {
                            debug!(
                                "{} [Session][{}] Sending packet for {} is too large ({} > max {}), fragmenting...",
                                peer_addr, session_id, address, len, max_payload_size
                            );
                            let fragment_id = fragment_id_counter.fetch_add(1, Ordering::Relaxed);
                            let fragments = UdpPacket::split_packet(session_id, address, data, max_payload_size, fragment_id);
                            for fragment in fragments {
                                if let Ok(encoded) = fragment.encode()
                                    && connection.send_datagram(encoded).await.is_err() { break; }
                            }
                        }
                    }
                }
            }
        }
    }
}
