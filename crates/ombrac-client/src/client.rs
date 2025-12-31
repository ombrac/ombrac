use std::io;
use std::sync::Arc;
#[cfg(feature = "datagram")]
use std::sync::atomic::AtomicU64;

use bytes::Bytes;
#[cfg(feature = "datagram")]
use tokio_util::sync::CancellationToken;

use ombrac::protocol::{Address, Secret};
use ombrac_macros::info;
use ombrac_transport::{Connection, Initiator};

use crate::connection::{ClientConnection, UdpDispatcher, UdpSession};

/// The central client responsible for managing the connection to the server.
///
/// This client handles TCP stream creation and delegates UDP session management
/// to a dedicated `UdpDispatcher`. It ensures the connection stays alive
/// through a retry mechanism.
pub struct Client<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    // The connection manager handles handshake, reconnection, and stream creation.
    connection: Arc<ClientConnection<T, C>>,
    // The handle to the background UDP dispatcher task.
    #[cfg(feature = "datagram")]
    _dispatcher_handle: tokio::task::JoinHandle<()>,
    #[cfg(feature = "datagram")]
    session_id_counter: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(feature = "datagram")]
    udp_dispatcher: Arc<UdpDispatcher>,
    #[cfg(feature = "datagram")]
    shutdown_token: CancellationToken,
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
        let connection = Arc::new(ClientConnection::new(transport, secret, options).await?);

        #[cfg(feature = "datagram")]
        let session_id_counter = Arc::new(AtomicU64::new(1));
        #[cfg(feature = "datagram")]
        let udp_dispatcher = Arc::new(UdpDispatcher::new());
        #[cfg(feature = "datagram")]
        let shutdown_token = CancellationToken::new();

        // Spawn the background task that reads all UDP datagrams and dispatches them.
        #[cfg(feature = "datagram")]
        let dispatcher_handle = {
            let connection_clone = Arc::clone(&connection);
            let dispatcher_clone = Arc::clone(&udp_dispatcher);
            let shutdown_clone = shutdown_token.clone();
            tokio::spawn(async move {
                UdpDispatcher::run(connection_clone, dispatcher_clone, shutdown_clone).await;
            })
        };

        Ok(Self {
            connection,
            #[cfg(feature = "datagram")]
            _dispatcher_handle: dispatcher_handle,
            #[cfg(feature = "datagram")]
            session_id_counter,
            #[cfg(feature = "datagram")]
            udp_dispatcher,
            #[cfg(feature = "datagram")]
            shutdown_token,
        })
    }

    /// Establishes a new UDP session through the tunnel.
    ///
    /// This returns a `UdpSession` object, which provides a socket-like API
    /// for sending and receiving UDP datagrams over the existing connection.
    #[cfg(feature = "datagram")]
    pub fn open_associate(&self) -> UdpSession<T, C> {
        let session_id = self
            .session_id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        info!(
            "[Client] New UDP session created with session_id={}",
            session_id
        );
        let receiver = self.udp_dispatcher.register_session(session_id);

        UdpSession::new(
            session_id,
            Arc::clone(&self.connection),
            Arc::clone(&self.udp_dispatcher),
            receiver,
        )
    }

    /// Opens a new bidirectional stream for TCP-like communication.
    ///
    /// This method negotiates a new stream with the server, which will then
    /// connect to the specified destination address. It waits for the server's
    /// connection response before returning, ensuring proper TCP state handling.
    pub async fn open_bidirectional(&self, dest_addr: Address) -> io::Result<C::Stream> {
        self.connection.open_bidirectional(dest_addr).await
    }

    /// Rebind the transport to a new socket to ensure a clean state for reconnection.
    pub async fn rebind(&self) -> io::Result<()> {
        self.connection.rebind().await
    }
}

impl<T, C> Drop for Client<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    fn drop(&mut self) {
        // Signal the background dispatcher to shut down when the client is dropped.
        #[cfg(feature = "datagram")]
        self.shutdown_token.cancel();
    }
}
