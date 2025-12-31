use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::{ArcSwap, Guard};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::codec::Framed;

use ombrac::codec::{DownstreamMessage, UpstreamMessage, length_codec};
use ombrac::protocol::{
    self, Address, ClientConnect, ClientHello, ConnectErrorKind, HandshakeError, PROTOCOLS_VERSION,
    Secret, ServerConnectResponse, ServerHandshakeResponse,
};
use ombrac_macros::{error, info, warn};
use ombrac_transport::{Connection, Initiator};

#[cfg(feature = "datagram")]
mod datagram;
mod stream;

// --- Handshake & Connection ---
/// Timeout for the initial handshake with the server [default: 10 seconds]
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum size for response messages [default: 8MB]
const MAX_RESPONSE_MESSAGE_SIZE: usize = 8 * 1024 * 1024;

// --- Reconnection Strategy ---
/// Initial backoff duration for reconnection attempts [default: 1 second]
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum backoff duration for reconnection attempts [default: 60 seconds]
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(60);

#[cfg(feature = "datagram")]
pub use datagram::{UdpDispatcher, UdpSession};

struct ReconnectState {
    last_attempt: Option<Instant>,
    backoff: Duration,
}

impl Default for ReconnectState {
    fn default() -> Self {
        Self {
            last_attempt: None,
            backoff: INITIAL_RECONNECT_BACKOFF,
        }
    }
}

/// Manages the connection to the server, including handshake and reconnection logic.
///
/// This struct handles the lifecycle of a connection to the server, including
/// initial handshake, automatic reconnection on failures, and connection state management.
pub struct ClientConnection<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    transport: T,
    connection: ArcSwap<C>,
    reconnect_lock: Mutex<ReconnectState>,
    secret: Secret,
    options: Bytes,
}

impl<T, C> ClientConnection<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    /// Creates a new `ClientConnection` and establishes a connection to the server.
    ///
    /// This involves performing a handshake with the server.
    pub async fn new(transport: T, secret: Secret, options: Option<Bytes>) -> io::Result<Self> {
        let options = options.unwrap_or_default();
        let connection = handshake(&transport, secret, options.clone()).await?;

        Ok(Self {
            transport,
            connection: ArcSwap::new(Arc::new(connection)),
            reconnect_lock: Mutex::new(ReconnectState::default()),
            secret,
            options,
        })
    }

    /// Opens a new bidirectional stream for TCP-like communication.
    ///
    /// This method negotiates a new stream with the server, which will then
    /// connect to the specified destination address. It waits for the server's
    /// connection response before returning, ensuring proper TCP state handling.
    pub async fn open_bidirectional(&self, dest_addr: Address) -> io::Result<C::Stream> {
        let mut stream = self
            .with_retry(|conn| async move { conn.open_bidirectional().await })
            .await?;

        let connect_message = UpstreamMessage::Connect(ClientConnect {
            address: dest_addr.clone(),
        });
        let encoded_bytes = protocol::encode(&connect_message)?;

        // The protocol requires a length-prefixed message.
        stream.write_u32(encoded_bytes.len() as u32).await?;
        stream.write_all(&encoded_bytes).await?;

        // Wait for the server's connection response
        // Read the length prefix manually to avoid borrowing issues
        let response_length = stream.read_u32().await?;
        if response_length > MAX_RESPONSE_MESSAGE_SIZE as u32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("response message too large: {} bytes", response_length),
            ));
        }

        let mut response_buffer = vec![0u8; response_length as usize];
        stream.read_exact(&mut response_buffer).await?;

        let message: DownstreamMessage = protocol::decode(&response_buffer)?;
        let response = match message {
            DownstreamMessage::ConnectResponse(response) => response,
            #[allow(unreachable_patterns)]
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected connect response message",
                ));
            }
        };

        // Handle the connection response
        match response {
            ServerConnectResponse::Ok => {
                // Connection successful - return the stream for data exchange
                Ok(stream)
            }
            ServerConnectResponse::Err { kind, message } => {
                // Connection failed - return appropriate error
                let error_kind = match kind {
                    ConnectErrorKind::ConnectionRefused => io::ErrorKind::ConnectionRefused,
                    ConnectErrorKind::TimedOut => io::ErrorKind::TimedOut,
                    ConnectErrorKind::NetworkUnreachable => io::ErrorKind::NetworkUnreachable,
                    ConnectErrorKind::HostUnreachable => io::ErrorKind::HostUnreachable,
                    ConnectErrorKind::Other => io::ErrorKind::Other,
                };
                Err(io::Error::new(
                    error_kind,
                    format!("failed to connect to {}: {}", dest_addr, message),
                ))
            }
        }
    }

    /// Gets a reference to the current connection.
    pub fn connection(&self) -> Guard<Arc<C>> {
        self.connection.load()
    }

    /// Rebind the transport to a new socket to ensure a clean state for reconnection.
    pub async fn rebind(&self) -> io::Result<()> {
        self.transport.rebind().await
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
                    error = %e,
                    error_kind = ?e.kind(),
                    "connection error detected, attempting to reconnect"
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
        let mut state = self.reconnect_lock.lock().await;

        let current_conn = self.connection.load();
        let current_conn_id = Arc::as_ptr(&current_conn) as usize;
        if current_conn_id != old_conn_id {
            return Ok(());
        }

        if let Some(last) = state.last_attempt {
            let elapsed = last.elapsed();
            if elapsed < state.backoff {
                let wait_time = state.backoff - elapsed;
                warn!(
                    wait_time_ms = wait_time.as_millis(),
                    backoff_secs = state.backoff.as_secs(),
                    "reconnect throttled, too many attempts"
                );

                drop(state);
                tokio::time::sleep(wait_time).await;
                return Err(io::Error::new(io::ErrorKind::Other, "reconnect throttled"));
            }
        }

        info!("attempting to reconnect to server");

        state.last_attempt = Some(Instant::now());

        if let Err(e) = self.transport.rebind().await {
            state.backoff = (state.backoff * 2).min(MAX_RECONNECT_BACKOFF);
            warn!(
                error = %e,
                next_backoff_secs = state.backoff.as_secs(),
                "transport rebind failed during reconnect"
            );
            return Err(e);
        }

        match handshake(&self.transport, self.secret, self.options.clone()).await {
            Ok(new_connection) => {
                state.backoff = INITIAL_RECONNECT_BACKOFF;
                state.last_attempt = None;

                self.connection.store(Arc::new(new_connection));
                info!("reconnection successful");
                Ok(())
            }
            Err(e) => {
                state.backoff = (state.backoff * 2).min(MAX_RECONNECT_BACKOFF);
                error!(
                    error = %e,
                    error_kind = ?e.kind(),
                    next_backoff_secs = state.backoff.as_secs(),
                    "reconnect handshake failed"
                );
                Err(e)
            }
        }
    }
}

/// Performs the initial handshake with the server.
async fn handshake<T, C>(transport: &T, secret: Secret, options: Bytes) -> io::Result<C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
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
                        info!("handshake with server successful");
                        stream.shutdown().await?;
                        Ok(connection)
                    }
                    ServerHandshakeResponse::Err(e) => {
                        let err_kind = match e {
                            HandshakeError::InvalidSecret => io::ErrorKind::PermissionDenied,
                            _ => io::ErrorKind::InvalidData,
                        };
                        error!(
                            error = ?e,
                            error_kind = ?err_kind,
                            "handshake failed, server rejected"
                        );
                        Err(io::Error::new(
                            err_kind,
                            format!("server rejected handshake: {:?}", e),
                        ))
                    }
                }
            }
            Some(Err(e)) => {
                error!(
                    error = %e,
                    error_kind = ?e.kind(),
                    "connection error during handshake"
                );
                Err(e)
            }
            None => {
                error!("connection closed by server during handshake");
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed by server during handshake",
                ))
            }
        }
    };

    match tokio::time::timeout(HANDSHAKE_TIMEOUT, do_handshake).await {
        Ok(result) => result,
        Err(_) => {
            error!(
                timeout_secs = HANDSHAKE_TIMEOUT.as_secs(),
                "handshake timed out"
            );
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "client hello timed out",
            ))
        }
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
