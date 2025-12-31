#[cfg(feature = "datagram")]
mod datagram;
mod stream;

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Weak;
use std::time::{Duration, Instant};

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
use ombrac_macros::{debug, error, info, warn};
use ombrac_transport::{Acceptor, Connection};

use crate::config::ConnectionConfig;

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
    /// This method is called during the handshake phase to verify the client's
    /// credentials. If verification succeeds, it returns a context that will
    /// be passed to `accept`.
    fn verify(
        &self,
        hello: &protocol::ClientHello,
    ) -> impl Future<Output = Result<Self::AuthContext, protocol::HandshakeError>> + Send;

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

    async fn verify(&self, hello: &protocol::ClientHello) -> Result<(), protocol::HandshakeError> {
        if &hello.secret == self {
            Ok(())
        } else {
            Err(protocol::HandshakeError::InvalidSecret)
        }
    }

    async fn accept(&self, _auth_context: Self::AuthContext, _connection: ConnectionHandle<T>) {
        ()
    }
}

/// Processes a single client connection, handling handshake and tunnel management.
///
/// This struct manages the lifecycle of a client connection after it has been
/// accepted by the server. It performs authentication, sets up tunnel handlers
/// for streams and datagrams, and manages the connection until it closes.
pub struct ClientConnectionProcessor<C: Connection> {
    transport_connection: Arc<C>,
    shutdown_token: CancellationToken,
}

impl<C: Connection> ClientConnectionProcessor<C> {
    /// Handles a new client connection from handshake through tunnel setup.
    ///
    /// This method:
    /// 1. Performs the handshake and authentication
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
            Self::perform_handshake(connection, authenticator, &config).await?;

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

    async fn perform_handshake<A>(
        connection: C,
        authenticator: &A,
        config: &ConnectionConfig,
    ) -> io::Result<(A::AuthContext, C)>
    where
        A: Authenticator<C>,
    {
        let handshake_timeout = Duration::from_secs(config.handshake_timeout_secs());

        let mut control_stream = connection.accept_bidirectional().await?;
        let mut control_frame = Framed::new(&mut control_stream, codec::length_codec());

        // Read hello message with timeout
        let payload = tokio::time::timeout(handshake_timeout, control_frame.next())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Handshake timeout: failed to receive hello message within {:?}",
                        handshake_timeout
                    ),
                )
            })?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "Stream closed before hello")
            })??;

        let message: codec::UpstreamMessage = protocol::decode(&payload)?;

        let hello = match message {
            codec::UpstreamMessage::Hello(h) => h,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Expected Hello message",
                ));
            }
        };

        #[cfg(feature = "tracing")]
        Self::trace_handshake(&hello);

        // Verify with timeout
        let auth_result = if hello.version != protocol::PROTOCOLS_VERSION {
            Err(protocol::HandshakeError::UnsupportedVersion)
        } else {
            match tokio::time::timeout(handshake_timeout, authenticator.verify(&hello)).await {
                Ok(result) => result,
                Err(_) => Err(protocol::HandshakeError::InternalServerError),
            }
        };

        // Send response with timeout
        let response = match &auth_result {
            Ok(_) => protocol::ServerHandshakeResponse::Ok,
            Err(e) => protocol::ServerHandshakeResponse::Err(e.clone()),
        };

        tokio::time::timeout(
            handshake_timeout,
            control_frame.send(protocol::encode(&response)?),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "Handshake timeout: failed to send response within {:?}",
                    handshake_timeout
                ),
            )
        })??;

        match auth_result {
            Ok(auth_context) => Ok((auth_context, connection)),
            Err(e) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Handshake failed: {:?}", e),
            )),
        }
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
            Ok(Ok(_)) => debug!("Connection closed gracefully."),
            Ok(Err(e)) => debug!("Connection closed with internal error: {}", e),
            Err(e) => warn!("Tunnel handler task panicked or failed: {}", e),
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
    fn trace_handshake(hello: &protocol::ClientHello) {
        let secret_hex = hello
            .secret
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        tracing::Span::current().record("secret", &secret_hex);
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
                    info!("Shutdown signal received, stopping accept loop");
                    break;
                },
                accepted = self.acceptor.accept() => {
                    match accepted {
                        Ok(connection) => {
                            let authenticator = Arc::clone(&self.authenticator);
                            let semaphore = Arc::clone(&self.connection_semaphore);
                            let config = Arc::clone(&self.config);

                            // Try to acquire an owned permit for this connection
                            // If semaphore is exhausted, we log and drop the connection
                            match semaphore.try_acquire_owned() {
                                Ok(permit) => {
                                    #[cfg(not(feature = "tracing"))]
                                    tokio::spawn(Self::process_connection_with_permit(
                                        connection,
                                        authenticator,
                                        permit,
                                        config,
                                    ));
                                    #[cfg(feature = "tracing")]
                                    tokio::spawn(Self::process_connection_with_permit(
                                        connection,
                                        authenticator,
                                        permit,
                                        config,
                                    ).in_current_span());
                                },
                                Err(_) => {
                                    warn!("Connection rejected: maximum concurrent connections ({}) reached",
                                        self.config.max_connections());
                                    // Connection is dropped here, which should trigger cleanup
                                }
                            }
                        },
                        Err(err) => {
                            error!("failed to accept connection: {}", err);
                            // Continue accepting even on errors
                        }
                    }
                },
            }
        }

        Ok(())
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
                secret = tracing::field::Empty
            )
        )
    )]
    async fn process_connection(
        connection: <T as Acceptor>::Connection,
        authenticator: Arc<A>,
        config: Arc<ConnectionConfig>,
    ) {
        #[cfg(feature = "tracing")]
        let created_at = Instant::now();

        let peer_addr = match connection.remote_address() {
            Ok(addr) => addr,
            Err(_err) => {
                return error!("failed to get remote address for incoming connection {_err}");
            }
        };

        #[cfg(feature = "tracing")]
        tracing::Span::current().record("from", tracing::field::display(peer_addr));

        let reason: std::borrow::Cow<'static, str> = {
            match ClientConnectionProcessor::handle(connection, authenticator.as_ref(), config)
                .await
            {
                Ok(_) => "ok".into(),
                Err(e) => {
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::TimedOut
                    ) {
                        format!("client disconnect: {}", e.kind()).into()
                    } else {
                        error!("connection processor failed: {e}");
                        format!("error: {e}").into()
                    }
                }
            }
        };

        #[cfg(feature = "tracing")]
        info!(
            duration = created_at.elapsed().as_millis(),
            reason = %reason.as_ref(),
            "connection closed"
        );
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.acceptor.local_addr()
    }
}
