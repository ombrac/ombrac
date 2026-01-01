#[cfg(feature = "datagram")]
mod datagram;
mod dns;
mod stream;

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Weak;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{Semaphore, broadcast};
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac::codec;
use ombrac::protocol;
use ombrac_macros::{debug, error, warn};
use ombrac_transport::{Acceptor, Connection};

use crate::config::ConnectionConfig;

/// Processes a single client connection, handling authentication and tunnel management.
///
/// This struct manages the lifecycle of a client connection after it has been
/// accepted by the server. It performs authentication, sets up tunnel handlers
/// for streams and datagrams, and manages the connection until it closes.
pub struct ClientConnectionProcessor<C: Connection> {
    transport_connection: Arc<C>,
    shutdown_token: CancellationToken,
}

impl<C: Connection> ClientConnectionProcessor<C> {
    /// Handles a new client connection from authentication through tunnel setup.
    ///
    /// This method:
    /// 1. Performs the authentication
    /// 2. Notifies the authenticator that the connection is accepted
    /// 3. Sets up and runs tunnel loops for streams and datagrams
    pub async fn handle<A>(
        connection: C,
        authenticator: &A,
        config: Arc<ConnectionConfig>,
    ) -> io::Result<()>
    where
        A: Authenticator<C>,
    {
        let (auth_context, connection) =
            Self::perform_authentication(connection, authenticator, &config).await?;

        let transport_connection = Arc::new(connection);

        authenticator
            .accept(
                auth_context,
                ConnectionHandle {
                    inner: transport_connection.clone(),
                },
            )
            .await;

        let processor = Self {
            transport_connection,
            shutdown_token: CancellationToken::new(),
        };

        processor.run_tunnel_loops().await;

        Ok(())
    }

    async fn perform_authentication<A: Authenticator<C>>(
        connection: C,
        authenticator: &A,
        config: &ConnectionConfig,
    ) -> io::Result<(A::AuthContext, C)> {
        let auth_timeout = Duration::from_secs(config.auth_timeout_secs());

        // Accept control stream
        let mut control_stream = connection.accept_bidirectional().await.map_err(|e| {
            io::Error::other(
                format!("failed to accept bidirectional stream: {}", e),
            )
        })?;
        let mut control_frame = Framed::new(&mut control_stream, codec::length_codec());

        // Read and parse hello message
        let hello = Self::read_hello_message(&mut control_frame, auth_timeout).await?;

        #[cfg(feature = "tracing")]
        Self::trace_auth(&hello);

        // Verify authentication
        let auth_context =
            Self::verify_authentication(&hello, authenticator, auth_timeout, &mut control_frame)
                .await?;

        Ok((auth_context, connection))
    }

    /// Reads and parses the hello message from the client.
    async fn read_hello_message(
        control_frame: &mut Framed<&mut <C as Connection>::Stream, codec::LengthDelimitedCodec>,
        timeout: Duration,
    ) -> io::Result<protocol::ClientHello>
    where
        C: Connection,
    {
        // Read payload with timeout
        let payload = tokio::time::timeout(timeout, control_frame.next())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "authentication timeout: failed to receive hello message within {:?}",
                        timeout
                    ),
                )
            })?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "stream closed before hello")
            })??;

        // Decode message
        let message: codec::ClientMessage = protocol::decode(&payload).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to decode client message: {}", e),
            )
        })?;

        // Extract hello message
        match message {
            codec::ClientMessage::Hello(hello) => Ok(hello),
            _ => {
                // Invalid message type - disconnect with random delay
                let stream = control_frame.get_mut();
                Self::disconnect_with_random_delay(*stream).await;
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authentication failed: invalid message type (expected Hello)",
                ))
            }
        }
    }

    /// Verifies authentication and sends response.
    async fn verify_authentication<A: Authenticator<C>>(
        hello: &protocol::ClientHello,
        authenticator: &A,
        timeout: Duration,
        control_frame: &mut Framed<&mut <C as Connection>::Stream, codec::LengthDelimitedCodec>,
    ) -> io::Result<A::AuthContext>
    where
        C: Connection,
    {
        // Check protocol version
        if hello.version != protocol::PROTOCOL_VERSION {
            Self::handle_auth_failure(control_frame).await;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "incompatible version",
            ));
        }

        // Perform authentication with timeout
        let auth_context = tokio::time::timeout(timeout, authenticator.verify(hello)).await??;

        Self::send_auth_ok_response(control_frame, timeout).await?;

        Ok(auth_context)
    }

    /// Sends authentication response with timeout.
    async fn send_auth_ok_response(
        control_frame: &mut Framed<&mut <C as Connection>::Stream, codec::LengthDelimitedCodec>,
        timeout: Duration,
    ) -> io::Result<()>
    where
        C: Connection,
    {
        tokio::time::timeout(
            timeout,
            control_frame.send(protocol::encode(&protocol::ServerAuthResponse::Ok)?),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "authentication timeout: failed to send response within {:?}",
                    timeout
                ),
            )
        })??;
        Ok(())
    }

    /// Handles authentication failure by disconnecting with random delay.
    async fn handle_auth_failure(
        control_frame: &mut Framed<&mut <C as Connection>::Stream, codec::LengthDelimitedCodec>,
    ) where
        C: Connection,
    {
        // Get the underlying stream for disconnection
        let stream = control_frame.get_mut();
        Self::disconnect_with_random_delay(*stream).await;
    }

    /// Disconnects the stream with a random delay to prevent timing attacks.
    ///
    /// This function introduces a random delay (100-500ms) before closing the stream,
    /// making it harder for attackers to distinguish between different failure modes
    /// based on response timing.
    async fn disconnect_with_random_delay(stream: &mut C::Stream) {
        use rand::Rng;

        let delay_ms = {
            let mut rng = rand::rng();
            rng.random_range(100..=500)
        };

        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let _ = tokio::io::AsyncWriteExt::shutdown(stream).await;
    }

    /// Runs the tunnel loops for streams and datagrams until the connection closes.
    async fn run_tunnel_loops(&self) {
        let stream_tunnel_handle = self.spawn_stream_tunnel();
        #[cfg(feature = "datagram")]
        let datagram_tunnel_handle = self.spawn_datagram_tunnel();

        #[cfg(not(feature = "datagram"))]
        let result = stream_tunnel_handle.await;

        #[cfg(feature = "datagram")]
        let result = tokio::select! {
            res = stream_tunnel_handle => res,
            res = datagram_tunnel_handle => res,
        };

        // Signal all related tasks to shut down
        self.shutdown_token.cancel();

        match result {
            Ok(Ok(_)) => debug!("connection closed gracefully"),
            Ok(Err(e)) => debug!("connection closed with internal error: {}", e),
            Err(e) => warn!("tunnel handler task panicked or failed: {}", e),
        }
    }

    fn spawn_stream_tunnel(&self) -> JoinHandle<io::Result<()>> {
        use crate::connection::stream::StreamTunnel;

        let connection = Arc::clone(&self.transport_connection);
        let shutdown = self.shutdown_token.child_token();
        let tunnel = StreamTunnel::new(connection, shutdown);

        #[cfg(not(feature = "tracing"))]
        let handle = tokio::spawn(tunnel.accept_loop());
        #[cfg(feature = "tracing")]
        let handle = tokio::spawn(tunnel.accept_loop().in_current_span());

        handle
    }

    #[cfg(feature = "datagram")]
    fn spawn_datagram_tunnel(&self) -> JoinHandle<io::Result<()>> {
        use crate::connection::datagram::DatagramTunnel;

        let connection = Arc::clone(&self.transport_connection);
        let shutdown = self.shutdown_token.child_token();
        let tunnel = DatagramTunnel::new(connection, shutdown);

        #[cfg(not(feature = "tracing"))]
        let handle = tokio::spawn(tunnel.accept_loop());
        #[cfg(feature = "tracing")]
        let handle = tokio::spawn(tunnel.accept_loop().in_current_span());

        handle
    }

    #[cfg(feature = "tracing")]
    fn trace_auth(hello: &protocol::ClientHello) {
        use std::io::Write;

        let mut buf = [0u8; 6];
        let mut cursor = std::io::Cursor::new(&mut buf[..]);

        for byte in hello.secret.iter().take(3) {
            let _ = write!(cursor, "{:02x}", byte);
        }

        if let Ok(hex_str) = std::str::from_utf8(&buf) {
            tracing::Span::current().record("secret", hex_str);
        }
    }
}

/// ConnectionAcceptor manages incoming connections with resource limits and lifecycle control.
///
/// This struct handles accepting new connections from the transport layer,
/// limiting concurrent connections, and delegating each connection to a processor.
///
/// Generic parameters:
/// - `T`: The acceptor type that accepts new connections from the transport
/// - `A`: The authenticator type that handles connection authentication
pub struct ConnectionAcceptor<T, A> {
    acceptor: Arc<T>,
    authenticator: Arc<A>,
    connection_semaphore: Arc<Semaphore>,
    config: Arc<ConnectionConfig>,
}

impl<T: Acceptor, A: Authenticator<T::Connection> + 'static> ConnectionAcceptor<T, A> {
    /// Creates a new connection acceptor with the given acceptor and authenticator.
    ///
    /// The acceptor will use default connection configuration if not provided.
    pub fn new(acceptor: T, authenticator: A) -> Self {
        Self::with_config(
            acceptor,
            authenticator,
            Arc::new(ConnectionConfig::default()),
        )
    }

    /// Creates a new connection acceptor with custom connection configuration.
    pub fn with_config(acceptor: T, authenticator: A, config: Arc<ConnectionConfig>) -> Self {
        let max_connections = config.max_connections();
        Self {
            acceptor: Arc::new(acceptor),
            authenticator: Arc::new(authenticator),
            connection_semaphore: Arc::new(Semaphore::new(max_connections)),
            config,
        }
    }

    /// Main accept loop that accepts incoming connections and manages them with resource limits.
    ///
    /// This method will:
    /// 1. Accept new connections from the acceptor
    /// 2. Acquire a semaphore permit to limit total concurrent connections
    /// 3. Spawn a task to handle each connection
    /// 4. Gracefully handle shutdown signals
    pub async fn accept_loop(&self, mut shutdown_rx: broadcast::Receiver<()>) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    break;
                },
                accepted = self.acceptor.accept() => {
                    Self::handle_incoming_connection(
                        accepted,
                        Arc::clone(&self.authenticator),
                        Arc::clone(&self.connection_semaphore),
                        Arc::clone(&self.config),
                    );
                },
            }
        }

        Ok(())
    }

    /// Handles an incoming connection, either spawning a processor or rejecting it.
    fn handle_incoming_connection(
        result: io::Result<<T as Acceptor>::Connection>,
        authenticator: Arc<A>,
        semaphore: Arc<Semaphore>,
        config: Arc<ConnectionConfig>,
    ) {
        match result {
            Ok(connection) => match semaphore.try_acquire_owned() {
                Ok(permit) => {
                    #[cfg(not(feature = "tracing"))]
                    tokio::spawn(Self::process_connection_with_permit(
                        connection,
                        authenticator,
                        permit,
                        config,
                    ));
                    #[cfg(feature = "tracing")]
                    tokio::spawn(
                        Self::process_connection_with_permit(
                            connection,
                            authenticator,
                            permit,
                            config,
                        )
                        .in_current_span(),
                    );
                }
                Err(_) => {
                    warn!(
                        "connection rejected: maximum concurrent connections ({}) reached",
                        config.max_connections()
                    );
                }
            },
            Err(err) => {
                error!("failed to accept connection: {}", err);
            }
        }
    }

    /// Processes a connection with a semaphore permit.
    ///
    /// The permit is automatically released when the connection is closed.
    async fn process_connection_with_permit(
        connection: <T as Acceptor>::Connection,
        authenticator: Arc<A>,
        _permit: OwnedSemaphorePermit,
        config: Arc<ConnectionConfig>,
    ) {
        // Permit is held for the lifetime of this function
        Self::process_connection(connection, authenticator, config).await;
        // Permit is automatically released when dropped
    }

    #[cfg_attr(feature = "tracing",
        tracing::instrument(
            name = "connection",
            skip_all,
            fields(
                id = connection.id(),
                from = tracing::field::Empty,
                secret = tracing::field::Empty,
                reason = tracing::field::Empty
            )
        )
    )]
    async fn process_connection(
        connection: <T as Acceptor>::Connection,
        authenticator: Arc<A>,
        config: Arc<ConnectionConfig>,
    ) {
        #[cfg(feature = "tracing")]
        if let Ok(addr) = connection.remote_address() {
            tracing::Span::current().record("from", tracing::field::display(addr));
        }

        let _result =
            ClientConnectionProcessor::handle(connection, authenticator.as_ref(), config).await;

        #[cfg(feature = "tracing")]
        match _result {
            Ok(_) => {
                tracing::Span::current().record("reason", "ok");
                tracing::info!("connection closed");
            }
            Err(e) => {
                tracing::Span::current().record("reason", tracing::field::display(&e));
                tracing::error!(error = %e, "connection closed with error");
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.acceptor.local_addr()
    }
}

pub struct ConnectionHandle<C> {
    inner: Arc<C>,
}

impl<C: Connection> ConnectionHandle<C> {
    pub fn downgrade_inner(&self) -> Weak<C> {
        Arc::downgrade(&self.inner)
    }

    pub fn close(&self, error_code: u32, reason: &[u8]) {
        self.inner.close(error_code, reason);
    }
}

/// Authentication error types returned by the server during authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionAuthError {
    /// Client protocol version is not supported by the server.
    IncompatibleVersion,
    /// Authentication secret is invalid or incorrect.
    InvalidSecret,
    /// Internal server error during authentication processing.
    ServerError,
    /// Other error
    Other(String),
}

impl From<ConnectionAuthError> for io::Error {
    fn from(value: ConnectionAuthError) -> Self {
        match value {
            ConnectionAuthError::IncompatibleVersion => {
                io::Error::new(io::ErrorKind::Unsupported, "incompatible protocol version")
            }
            ConnectionAuthError::InvalidSecret => io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid authentication secret",
            ),
            ConnectionAuthError::ServerError => io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "internal server error during auth",
            ),
            ConnectionAuthError::Other(msg) => io::Error::other(msg),
        }
    }
}

/// Authenticator trait for verifying and accepting client connections.
///
/// This trait provides authentication logic for incoming connections.
/// Implementations should verify the client's credentials and optionally
/// perform any setup needed when a connection is accepted.
pub trait Authenticator<T>: Send + Sync {
    /// Context type that can be passed from verification to acceptance.
    type AuthContext: Send;

    /// Verifies the client's hello message and returns an authentication context.
    ///
    /// This method is called during the authentication phase to verify the client's
    /// credentials. If verification succeeds, it returns a context that will
    /// be passed to `accept`.
    fn verify(
        &self,
        hello: &protocol::ClientHello,
    ) -> impl Future<Output = Result<Self::AuthContext, ConnectionAuthError>> + Send;

    /// Called after successful authentication to handle the accepted connection.
    ///
    /// This method is called with the authentication context from `verify` and
    /// a handle to the connection. Implementations can use this to perform any
    /// additional setup or logging.
    fn accept(
        &self,
        auth_context: Self::AuthContext,
        connection: ConnectionHandle<T>,
    ) -> impl Future<Output = ()> + Send;
}

impl<T: Send + Sync> Authenticator<T> for ombrac::protocol::Secret {
    type AuthContext = ();

    async fn verify(&self, hello: &protocol::ClientHello) -> Result<(), ConnectionAuthError> {
        if &hello.secret == self {
            Ok(())
        } else {
            Err(ConnectionAuthError::InvalidSecret)
        }
    }

    async fn accept(&self, _auth_context: Self::AuthContext, _connection: ConnectionHandle<T>) {
        
    }
}
