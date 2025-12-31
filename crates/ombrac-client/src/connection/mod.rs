use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::{ArcSwap, Guard};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::codec::Framed;

use ombrac::codec::{ClientMessage, ServerMessage, length_codec};
use ombrac::protocol::{
    self, Address, ClientConnect, ClientHello, ConnectErrorKind, PROTOCOL_VERSION, Secret,
    ServerAuthResponse, ServerConnectResponse,
};
use ombrac_macros::{error, info, warn};
use ombrac_transport::{Connection, Initiator};

#[cfg(feature = "datagram")]
mod datagram;
mod stream;

pub use stream::BufferedStream;

// --- Authentication & Connection ---
/// Timeout for the initial authentication with the server [default: 10 seconds]
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Manages the connection to the server, including authentication and reconnection logic.
///
/// This struct handles the lifecycle of a connection to the server, including
/// initial authentication, automatic reconnection on failures, and connection state management.
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
    /// This involves performing authentication with the server.
    pub async fn new(transport: T, secret: Secret, options: Option<Bytes>) -> io::Result<Self> {
        let options = options.unwrap_or_default();
        let connection = authenticate(&transport, secret, options.clone()).await?;

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
    ///
    /// The returned stream is wrapped in a `BufferedStream` to ensure that any
    /// data remaining in the protocol framing buffer is read first, preventing
    /// data loss when transitioning from message-based to raw stream communication.
    pub async fn open_bidirectional(
        &self,
        dest_addr: Address,
    ) -> io::Result<BufferedStream<C::Stream>> {
        let mut stream = self
            .with_retry(|conn| async move { conn.open_bidirectional().await })
            .await?;

        // Use Framed codec for consistent message framing
        let mut framed = Framed::new(&mut stream, length_codec());

        // Send connection request
        let connect_message = ClientMessage::Connect(ClientConnect {
            address: dest_addr.clone(),
        });
        framed.send(protocol::encode(&connect_message)?).await?;

        // Wait for the server's connection response
        // Framed automatically reads the length prefix and validates frame size
        let payload = match framed.next().await {
            Some(Ok(payload)) => payload,
            Some(Err(e)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to read server response: {}", e),
                ));
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream closed before receiving server response",
                ));
            }
        };

        let message: ServerMessage = protocol::decode(&payload)?;
        let response = match message {
            ServerMessage::ConnectResponse(response) => response,
            #[allow(unreachable_patterns)]
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected connect response message",
                ));
            }
        };

        // Extract any buffered data from Framed before dropping it
        // This ensures we don't lose any data that might be in the read/write buffers.
        // In this request-response protocol, we expect:
        // - read_buf to be empty (we've read the complete response)
        // - write_buf to be empty (send() should have flushed the request)
        let parts = framed.into_parts();

        // Verify write buffer is empty (send() should have flushed, but verify for safety)
        if !parts.write_buf.is_empty() {
            // This indicates send() didn't complete properly - this is a serious error
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "write buffer not empty after send: {} bytes remaining - data may be lost",
                    parts.write_buf.len()
                ),
            ));
        }

        // Extract any remaining buffered read data
        // This data may be present if the server sent additional data immediately after
        // the connection response. We preserve it by wrapping the stream in BufferedStream.
        let buffered_data = if !parts.read_buf.is_empty() {
            info!(
                buffered_bytes = parts.read_buf.len(),
                "preserving buffered data from protocol framing buffer"
            );
            Bytes::copy_from_slice(&parts.read_buf)
        } else {
            Bytes::new()
        };

        match response {
            ServerConnectResponse::Ok => {
                // Connection successful - return the stream wrapped in BufferedStream
                // to ensure any buffered data is read first
                Ok(BufferedStream::new(stream, buffered_data))
            }
            ServerConnectResponse::Err { kind, message } => {
                // Connection failed - return appropriate error
                let error_kind = match kind {
                    ConnectErrorKind::ConnectionRefused => io::ErrorKind::ConnectionRefused,
                    ConnectErrorKind::NetworkUnreachable => io::ErrorKind::NetworkUnreachable,
                    ConnectErrorKind::HostUnreachable => io::ErrorKind::HostUnreachable,
                    ConnectErrorKind::TimedOut => io::ErrorKind::TimedOut,
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
    ///
    /// # Errors
    ///
    /// Returns the original error if it's not a connection error, or the error
    /// from the retry attempt if reconnection fails.
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
                // Attempt reconnection - if it fails, return the reconnection error
                self.reconnect(old_conn_id).await?;
                // Retry the operation with the new connection
                let new_connection = self.connection.load();
                operation(new_connection).await
            }
            Err(e) => Err(e),
        }
    }

    /// Handles the reconnection logic with exponential backoff.
    ///
    /// It uses a mutex to prevent multiple tasks from trying to reconnect simultaneously.
    /// If another task has already reconnected, this function returns immediately.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Reconnection is throttled (too many attempts)
    /// - Transport rebind fails
    /// - Authentication fails
    async fn reconnect(&self, old_conn_id: usize) -> io::Result<()> {
        let mut state = self.reconnect_lock.lock().await;

        // Check if another task has already reconnected
        let current_conn = self.connection.load();
        let current_conn_id = Arc::as_ptr(&current_conn) as usize;
        if current_conn_id != old_conn_id {
            // Another task already reconnected, we're done
            return Ok(());
        }

        // Apply exponential backoff if we've attempted recently
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

        match authenticate(&self.transport, self.secret, self.options.clone()).await {
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
                    "reconnect authentication failed"
                );
                Err(e)
            }
        }
    }
}

/// Performs the initial authentication with the server.
async fn authenticate<T, C>(transport: &T, secret: Secret, options: Bytes) -> io::Result<C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    let do_auth = async {
        let connection = transport.connect().await?;
        let mut stream = connection.open_bidirectional().await?;

        let hello_message = ClientMessage::Hello(ClientHello {
            version: PROTOCOL_VERSION,
            secret,
            options,
        });

        let encoded_bytes = protocol::encode(&hello_message)?;
        let mut framed = Framed::new(&mut stream, length_codec());

        framed.send(encoded_bytes).await?;

        match framed.next().await {
            Some(Ok(payload)) => {
                let response: ServerAuthResponse = protocol::decode(&payload)?;
                match response {
                    ServerAuthResponse::Ok => {
                        stream.shutdown().await?;
                        Ok(connection)
                    }
                    ServerAuthResponse::Err => {
                        Err(io::Error::other(format!("authentication failed")))
                    }
                }
            }
            Some(Err(e)) => {
                error!(
                    error = %e,
                    error_kind = ?e.kind(),
                    "connection error during authentication"
                );
                Err(e)
            }
            None => {
                error!("connection closed by server during authentication");
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed by server during authentication",
                ))
            }
        }
    };

    match tokio::time::timeout(AUTH_TIMEOUT, do_auth).await {
        Ok(result) => result,
        Err(_) => {
            error!(
                timeout_secs = AUTH_TIMEOUT.as_secs(),
                "authentication timed out"
            );
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "client authentication timed out",
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
