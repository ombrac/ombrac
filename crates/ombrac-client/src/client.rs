use std::future::Future;
use std::io;
use std::sync::Arc;
#[cfg(feature = "datagram")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::{ArcSwap, Guard};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio_util::codec::Framed;
#[cfg(feature = "datagram")]
use tokio_util::sync::CancellationToken;

use ombrac::codec::{UpstreamMessage, length_codec};
use ombrac::protocol::{
    self, Address, ClientConnect, ClientHello, HandshakeError, PROTOCOLS_VERSION, Secret,
    ServerHandshakeResponse,
};
use ombrac_macros::{error, info, warn};
use ombrac_transport::{Connection, Initiator};

#[cfg(feature = "datagram")]
use datagram::dispatcher::UdpDispatcher;
#[cfg(feature = "datagram")]
pub use datagram::session::UdpSession;

/// The central client responsible for managing the connection to the server.
///
/// This client handles TCP stream creation and delegates UDP session management
/// to a dedicated `UdpDispatcher`. It ensures the connection stays alive
/// through a retry mechanism.
pub struct Client<T, C> {
    // Inner state is Arc'd to be shared with background tasks and UDP sessions.
    inner: Arc<ClientInner<T, C>>,
    // The handle to the background UDP dispatcher task.
    #[cfg(feature = "datagram")]
    _dispatcher_handle: tokio::task::JoinHandle<()>,
}

/// The shared inner state of the `Client`.
///
/// This struct holds all the components necessary for the client's operation,
/// such as the transport, the current connection, and session management state.
pub(crate) struct ClientInner<T, C> {
    pub(crate) transport: T,
    pub(crate) connection: ArcSwap<C>,
    // A lock to ensure only one task attempts to reconnect at a time.
    reconnect_lock: Mutex<()>,
    secret: Secret,
    options: Bytes,
    #[cfg(feature = "datagram")]
    session_id_counter: AtomicU64,
    #[cfg(feature = "datagram")]
    pub(crate) udp_dispatcher: UdpDispatcher,
    // Token to signal all background tasks to shut down gracefully.
    #[cfg(feature = "datagram")]
    pub(crate) shutdown_token: CancellationToken,
}

impl<T, C> Client<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    /// Creates a new `Client` and establishes a connection to the server.
    ///
    /// This involves performing a handshake and spawning a background task to
    /// handle incoming UDP datagrams.
    pub async fn new(transport: T, secret: Secret, options: Option<Bytes>) -> io::Result<Self> {
        let options = options.unwrap_or_default();
        let connection = handshake(&transport, secret, options.clone()).await?;

        let inner = Arc::new(ClientInner {
            transport,
            connection: ArcSwap::new(Arc::new(connection)),
            reconnect_lock: Mutex::new(()),
            secret,
            options,
            #[cfg(feature = "datagram")]
            session_id_counter: AtomicU64::new(1),
            #[cfg(feature = "datagram")]
            udp_dispatcher: UdpDispatcher::new(),
            #[cfg(feature = "datagram")]
            shutdown_token: CancellationToken::new(),
        });

        // Spawn the background task that reads all UDP datagrams and dispatches them.
        #[cfg(feature = "datagram")]
        let dispatcher_handle = tokio::spawn(UdpDispatcher::run(Arc::clone(&inner)));

        Ok(Self {
            inner,
            #[cfg(feature = "datagram")]
            _dispatcher_handle: dispatcher_handle,
        })
    }

    /// Establishes a new UDP session through the tunnel.
    ///
    /// This returns a `UdpSession` object, which provides a socket-like API
    /// for sending and receiving UDP datagrams over the existing connection.
    #[cfg(feature = "datagram")]
    pub fn open_associate(&self) -> UdpSession<T, C> {
        let session_id = self.inner.new_session_id();
        info!(
            "[Client] New UDP session created with session_id={}",
            session_id
        );
        let receiver = self.inner.udp_dispatcher.register_session(session_id);

        UdpSession::new(session_id, Arc::clone(&self.inner), receiver)
    }

    /// Opens a new bidirectional stream for TCP-like communication.
    ///
    /// This method negotiates a new stream with the server, which will then
    /// connect to the specified destination address.
    pub async fn open_bidirectional(&self, dest_addr: Address) -> io::Result<C::Stream> {
        let mut stream = self
            .inner
            .with_retry(|conn| async move { conn.open_bidirectional().await })
            .await?;

        let connect_message = UpstreamMessage::Connect(ClientConnect { address: dest_addr });
        let encoded_bytes = protocol::encode(&connect_message)?;

        // The protocol requires a length-prefixed message.
        stream.write_u32(encoded_bytes.len() as u32).await?;
        stream.write_all(&encoded_bytes).await?;

        Ok(stream)
    }

    // Rebind the transport to a new socket to ensure a clean state for reconnection.
    pub async fn rebind(&self) -> io::Result<()> {
        self.inner.transport.rebind().await
    }
}

impl<T, C> Drop for Client<T, C> {
    fn drop(&mut self) {
        // Signal the background dispatcher to shut down when the client is dropped.
        #[cfg(feature = "datagram")]
        self.inner.shutdown_token.cancel();
    }
}

// --- Internal Implementation ---

impl<T, C> ClientInner<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    /// Atomically generates a new unique session ID.
    #[cfg(feature = "datagram")]
    pub(crate) fn new_session_id(&self) -> u64 {
        self.session_id_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// A wrapper function that adds retry/reconnect logic to a connection operation.
    ///
    /// It executes the provided `operation`. If the operation fails with a
    /// connection-related error, it attempts to reconnect and retries the
    /// operation once.
    pub(crate) async fn with_retry<F, Fut, R>(&self, operation: F) -> io::Result<R>
    where
        F: Fn(Guard<Arc<C>>) -> Fut,
        Fut: Future<Output = io::Result<R>>,
    {
        let connection = self.connection.load();
        // Use the pointer address as a unique ID for the connection instance.
        let old_conn_id = Arc::as_ptr(&connection) as usize;

        match operation(connection).await {
            Ok(result) => Ok(result),
            Err(e) if is_connection_error(&e) => {
                warn!(
                    "Connection error detected: {}. Attempting to reconnect...",
                    e
                );
                self.reconnect(old_conn_id).await?;
                let new_connection = self.connection.load();
                operation(new_connection).await
            }
            Err(e) => Err(e),
        }
    }

    /// Handles the reconnection logic.
    ///
    /// It uses a mutex to prevent multiple tasks from trying to reconnect simultaneously.
    async fn reconnect(&self, old_conn_id: usize) -> io::Result<()> {
        let _lock = self.reconnect_lock.lock().await;

        let current_conn_id = Arc::as_ptr(&self.connection.load()) as usize;
        // Check if another task has already reconnected.
        if current_conn_id == old_conn_id {
            // Rebind the transport to a new socket to ensure a clean state for reconnection.
            self.transport.rebind().await?;

            let new_connection =
                handshake(&self.transport, self.secret, self.options.clone()).await?;
            self.connection.store(Arc::new(new_connection));
            info!("Reconnection successful");
        }

        Ok(())
    }
}

/// Performs the initial handshake with the server.
async fn handshake<T, C>(transport: &T, secret: Secret, options: Bytes) -> io::Result<C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

    let do_handshake = async {
        let connection = transport.connect().await?;
        let mut stream = connection.open_bidirectional().await?;

        let hello_message = UpstreamMessage::Hello(ClientHello {
            version: PROTOCOLS_VERSION,
            secret,
            options,
        });

        let encoded_bytes = protocol::encode(&hello_message)?;
        let mut framed = Framed::new(&mut stream, length_codec());

        framed.send(encoded_bytes).await?;

        match framed.next().await {
            Some(Ok(payload)) => {
                let response: ServerHandshakeResponse = protocol::decode(&payload)?;
                match response {
                    ServerHandshakeResponse::Ok => {
                        info!("Handshake with server successful");
                        stream.shutdown().await?;
                        Ok(connection)
                    }
                    ServerHandshakeResponse::Err(e) => {
                        error!("Handshake failed: {:?}", e);
                        let err_kind = match e {
                            HandshakeError::InvalidSecret => io::ErrorKind::PermissionDenied,
                            _ => io::ErrorKind::InvalidData,
                        };
                        Err(io::Error::new(
                            err_kind,
                            format!("Server rejected handshake: {:?}", e),
                        ))
                    }
                }
            }
            Some(Err(e)) => Err(e),
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Connection closed by server during handshake",
            )),
        }
    };

    match tokio::time::timeout(HANDSHAKE_TIMEOUT, do_handshake).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Client hello timed out",
        )),
    }
}

/// Checks if an `io::Error` is related to a lost connection.
fn is_connection_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
            | io::ErrorKind::TimedOut
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::NetworkUnreachable
    )
}

/// The `datagram` module encapsulates all logic related to handling UDP datagrams,
/// including session management, packet fragmentation, and reassembly.
#[cfg(feature = "datagram")]
mod datagram {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use bytes::Bytes;
    use ombrac::protocol::{Address, UdpPacket};
    use ombrac::reassembly::UdpReassembler;
    use ombrac_macros::{debug, warn};
    use ombrac_transport::{Connection, Initiator};

    use super::ClientInner;

    /// Sends a UDP datagram, handling fragmentation if necessary.
    pub(crate) async fn send_datagram<T, C>(
        inner: &ClientInner<T, C>,
        session_id: u64,
        dest_addr: Address,
        data: Bytes,
        fragment_id_counter: &AtomicU32,
    ) -> io::Result<()>
    where
        T: Initiator<Connection = C>,
        C: Connection,
    {
        if data.is_empty() {
            return Ok(());
        }

        let connection = inner.connection.load();
        // Use a conservative default MTU if the transport doesn't provide one.
        let max_datagram_size = connection.max_datagram_size().unwrap_or(1350);
        // Leave a reasonable margin for headers.
        let overhead = UdpPacket::fragmented_overhead();
        let max_payload_size = max_datagram_size.saturating_sub(overhead).max(1);

        if data.len() <= max_payload_size {
            let packet = UdpPacket::Unfragmented {
                session_id,
                address: dest_addr.clone(),
                data,
            };
            let encoded = packet.encode()?;
            inner
                .with_retry(|conn| {
                    let data_for_attempt = encoded.clone();
                    async move { conn.send_datagram(data_for_attempt).await }
                })
                .await?;
        } else {
            // The packet is too large and must be fragmented.
            debug!(
                "[Session][{}] Sending packet for {} is too large ({} > max {}), fragmenting...",
                session_id,
                dest_addr,
                data.len(),
                max_payload_size
            );

            let fragment_id = fragment_id_counter.fetch_add(1, Ordering::Relaxed);
            let fragments =
                UdpPacket::split_packet(session_id, dest_addr, data, max_payload_size, fragment_id);

            for fragment in fragments {
                let packet_bytes = fragment.encode()?;
                inner
                    .with_retry(|conn| {
                        let data_for_attempt = packet_bytes.clone();
                        async move { conn.send_datagram(data_for_attempt).await }
                    })
                    .await?;
            }
        }
        Ok(())
    }

    /// Reads a UDP datagram from the connection, handling reassembly.
    pub(crate) async fn read_datagram<T, C>(
        inner: &ClientInner<T, C>,
        reassembler: &mut UdpReassembler,
    ) -> io::Result<(u64, Address, Bytes)>
    where
        T: Initiator<Connection = C>,
        C: Connection,
    {
        loop {
            let packet_bytes = inner
                .with_retry(|conn| async move { conn.read_datagram().await })
                .await?;

            let packet = match UdpPacket::decode(&packet_bytes) {
                Ok(packet) => packet,
                Err(_e) => {
                    warn!("Failed to decode UDP packet: {}. Discarding.", _e);
                    continue; // Skip malformed packets.
                }
            };

            match reassembler.process(packet).await {
                Ok(Some((session_id, address, data))) => {
                    return Ok((session_id, address, data));
                }
                Ok(None) => {
                    continue; // Fragment received, continue reading.
                }
                Err(_e) => {
                    warn!("Reassembly error: {}. Discarding fragment.", _e);
                    continue; // Reassembly error, wait for the next valid packet.
                }
            }
        }
    }

    /// Contains the `UdpDispatcher` which runs as a background task.
    pub(crate) mod dispatcher {
        use super::*;
        use dashmap::DashMap;
        use tokio::sync::mpsc;

        type UdpSessionSender = mpsc::Sender<(Bytes, Address)>;

        /// Manages all active UDP sessions and dispatches incoming datagrams.
        pub(crate) struct UdpDispatcher {
            // Maps a session_id to a sender that forwards data to the `UdpSession`.
            dispatch_map: DashMap<u64, UdpSessionSender>,
            fragment_id_counter: AtomicU32,
        }

        impl UdpDispatcher {
            pub(crate) fn new() -> Self {
                Self {
                    dispatch_map: DashMap::new(),
                    fragment_id_counter: AtomicU32::new(0),
                }
            }

            /// The main loop for the background UDP dispatcher task.
            ///
            /// It continuously reads datagrams from the server, reassembles them,
            /// and forwards them to the correct `UdpSession`.
            pub(crate) async fn run<T, C>(inner: Arc<ClientInner<T, C>>)
            where
                T: Initiator<Connection = C>,
                C: Connection,
            {
                let mut reassembler = UdpReassembler::default();

                loop {
                    tokio::select! {
                        // Listen for the shutdown signal.
                        _ = inner.shutdown_token.cancelled() => {
                            break;
                        }
                        // Read the next datagram from the server.
                        result = read_datagram(&inner, &mut reassembler) => {
                            match result {
                                Ok((session_id, address, data)) => {
                                    inner.udp_dispatcher.dispatch(session_id, data, address).await;
                                }
                                Err(_e) => {
                                    warn!("Error reading datagram: {}. Retrying after delay...", _e);
                                     // A small delay to prevent a tight loop on persistent errors.
                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                }
                            }
                        }
                    }
                }
            }

            /// Forwards a received datagram to the appropriate session.
            async fn dispatch(&self, session_id: u64, data: Bytes, address: Address) {
                if let Some(tx) = self.dispatch_map.get(&session_id) {
                    // If sending fails, the receiver (`UdpSession`) has been dropped.
                    // It's safe to clean up the entry from the map.
                    if tx.send((data, address)).await.is_err() {
                        self.dispatch_map.remove(&session_id);
                    }
                } else {
                    warn!(
                        "[Session][{}] Received datagram for UNKNOWN or CLOSED",
                        session_id
                    );
                }
            }

            /// Registers a new session and returns a receiver for it.
            pub(crate) fn register_session(
                &self,
                session_id: u64,
            ) -> mpsc::Receiver<(Bytes, Address)> {
                let (tx, rx) = mpsc::channel(128); // Channel buffer size
                self.dispatch_map.insert(session_id, tx);
                rx
            }

            /// Unregisters a session when it is dropped.
            pub(crate) fn unregister_session(&self, session_id: u64) {
                self.dispatch_map.remove(&session_id);
            }

            /// Provides access to the fragment ID counter for sending datagrams.
            pub(crate) fn fragment_id_counter(&self) -> &AtomicU32 {
                &self.fragment_id_counter
            }
        }
    }

    /// Represents a virtual UDP session over the tunnel.
    pub mod session {
        use super::*;
        use crate::client::datagram::ClientInner;
        use tokio::sync::mpsc;

        /// A virtual UDP session that provides a socket-like API.
        ///
        /// When this struct is dropped, its session is automatically cleaned up
        /// on the client side to prevent resource leaks.
        pub struct UdpSession<T, C>
        where
            T: Initiator<Connection = C>,
            C: Connection,
        {
            session_id: u64,
            client_inner: Arc<ClientInner<T, C>>,
            receiver: mpsc::Receiver<(Bytes, Address)>,
        }

        impl<T, C> UdpSession<T, C>
        where
            T: Initiator<Connection = C>,
            C: Connection,
        {
            /// Creates a new `UdpSession`. This is called by `Client::new_udp_session`.
            pub(crate) fn new(
                session_id: u64,
                client_inner: Arc<ClientInner<T, C>>,
                receiver: mpsc::Receiver<(Bytes, Address)>,
            ) -> Self {
                Self {
                    session_id,
                    client_inner,
                    receiver,
                }
            }

            /// Sends a UDP datagram to the specified destination through the tunnel.
            pub async fn send_to(&self, data: Bytes, dest_addr: Address) -> io::Result<()> {
                send_datagram(
                    &self.client_inner,
                    self.session_id,
                    dest_addr,
                    data,
                    self.client_inner.udp_dispatcher.fragment_id_counter(),
                )
                .await
            }

            /// Receives a UDP datagram from the tunnel for this session.
            ///
            /// Returns the received data and its original sender address.
            pub async fn recv_from(&mut self) -> Option<(Bytes, Address)> {
                self.receiver.recv().await
            }
        }

        impl<T, C> Drop for UdpSession<T, C>
        where
            T: Initiator<Connection = C>,
            C: Connection,
        {
            fn drop(&mut self) {
                // When a session is dropped, remove its dispatcher from the map
                // to prevent the map from growing indefinitely.
                self.client_inner
                    .udp_dispatcher
                    .unregister_session(self.session_id);
            }
        }
    }
}
