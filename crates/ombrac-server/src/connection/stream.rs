use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac::{codec, protocol};
use ombrac_macros::info;
use ombrac_transport::Connection;
use ombrac_transport::io::{CopyBidirectionalStats, copy_bidirectional};

use crate::connection::dns;

const MAX_CONCURRENT_CONNECTIONS: usize = 4096;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct StreamTunnel<C: Connection> {
    connection: Arc<C>,
    shutdown: CancellationToken,
    semaphore: Arc<Semaphore>,
}

impl<C: Connection> StreamTunnel<C> {
    pub(crate) fn new(connection: Arc<C>, shutdown: CancellationToken) -> Self {
        Self {
            connection,
            shutdown,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
        }
    }

    pub(crate) async fn accept_loop(self) -> io::Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => break,
                result = self.connection.accept_bidirectional() => {
                    let stream = result?;
                    let semaphore = Arc::clone(&self.semaphore);
                    let shutdown = self.shutdown.child_token();

                    let future = async move {
                        // Acquire semaphore permit to limit concurrent connections
                        let _permit = match semaphore.acquire().await {
                            Ok(permit) => permit,
                            Err(_) => {
                                // Semaphore is closed, skip this connection
                                return;
                            }
                        };

                        let mut guard = StreamGuard::default();
                        if let Err(e) = Self::handle_connect(stream, &mut guard, shutdown).await {
                            guard.reason = Some(e);
                        }
                        // Permit is automatically released when dropped
                    };

                    #[cfg(not(feature = "tracing"))]
                    tokio::spawn(future);
                    #[cfg(feature = "tracing")]
                    tokio::spawn(future.in_current_span());
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn handle_connect(
        mut stream: C::Stream,
        guard: &mut StreamGuard,
        shutdown: CancellationToken,
    ) -> io::Result<()> {
        let mut framed = Framed::new(&mut stream, codec::length_codec());

        // Step 1: Read the connection request from the client (with timeout)
        let destination = Self::read_connect_message(&mut framed).await?;
        guard.destination = Some(destination.clone());

        // Step 2: Attempt to connect to the destination (with timeout)
        let connect_result = Self::connect_to_destination(&destination).await;

        // Step 3: Send connection response to client
        // This must happen before we proceed, so the client knows the connection status
        Self::send_connect_response(&mut framed, &connect_result).await?;

        // Step 4: If connection failed, return error (client has already been notified)
        let mut tcp_stream = connect_result?;

        // Step 5: Exchange data between client and destination
        // Note: This phase has no timeout as it's the normal data transfer phase
        // that can last for the entire lifetime of the TCP connection.
        // Graceful shutdown is supported via tokio::select!
        Self::exchange_data(framed, &mut tcp_stream, guard, shutdown).await
    }

    /// Reads the connect message from the client.
    ///
    /// This function includes a timeout to prevent hanging on unresponsive clients.
    async fn read_connect_message(
        framed: &mut Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
    ) -> io::Result<protocol::Address> {
        let payload = tokio::time::timeout(HANDSHAKE_TIMEOUT, framed.next())
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "timeout reading connect message")
            })?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "stream closed unexpectedly")
            })??;

        match protocol::decode(&payload)? {
            codec::ClientMessage::Connect(connect) => Ok(connect.address),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected connect message",
            )),
        }
    }

    /// Attempts to connect to the destination address with a timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if DNS resolution fails or the connection times out.
    async fn connect_to_destination(destination: &protocol::Address) -> io::Result<TcpStream> {
        let addr = match destination {
            protocol::Address::SocketV4(addr) => SocketAddr::V4(*addr),
            protocol::Address::SocketV6(addr) => SocketAddr::V6(*addr),
            protocol::Address::Domain(domain, port) => {
                // Use shared DNS resolver for DNS resolution
                dns::resolve_domain(domain, *port).await?
            }
        };

        tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connection timeout"))?}

    /// Sends the connection response to the client.
    ///
    /// This function sends either a success or error response based on the connection result.
    async fn send_connect_response(
        framed: &mut Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
        connect_result: &io::Result<TcpStream>,
    ) -> io::Result<()> {
        let response = match connect_result {
            Ok(_) => {
                // Connection successful
                protocol::ServerConnectResponse::Ok
            }
            Err(e) => {
                // Connection failed - send error response
                // Use protocol layer error conversion for consistent error handling
                let error_kind = protocol::ConnectErrorKind::from_io_error(e);
                let error_message = e.to_string();
                protocol::ServerConnectResponse::Err {
                    kind: error_kind,
                    message: error_message,
                }
            }
        };

        let response_message = codec::ServerMessage::ConnectResponse(response);
        let encoded_response = protocol::encode(&response_message)?;
        framed.send(encoded_response).await?;

        Ok(())
    }

    /// Exchanges data between the client stream and the destination TCP stream.
    ///
    /// This function uses tokio::select! to listen for shutdown signals during
    /// data exchange, allowing graceful shutdown of active connections.
    async fn exchange_data(
        framed: Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
        tcp_stream: &mut TcpStream,
        guard: &mut StreamGuard,
        shutdown: CancellationToken,
    ) -> io::Result<()> {
        // Extract any buffered data from the framed stream
        let parts = framed.into_parts();
        guard.initial_upstream_bytes = parts.read_buf.len() as u64;

        // Write any buffered upstream data to the destination
        if !parts.read_buf.is_empty() {
            tcp_stream.write_all(&parts.read_buf).await?;
        }

        // Perform bidirectional data copying with graceful shutdown support
        let mut stream_inner = parts.io;

        // Use tokio::select! to listen for shutdown signal during data exchange
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                // Shutdown signal received, gracefully close the connection
                // Return a clean error to indicate shutdown
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "connection closed due to active closure"
                ))
            }
            result = copy_bidirectional(&mut stream_inner, tcp_stream) => {
                match result {
                    Ok(stats) => {
                        guard.stats = Some(stats);
                        Ok(())
                    }
                    Err((e, stats)) => {
                        guard.stats = Some(stats);
                        Err(e)
                    }
                }
            }
        }
    }
}

pub(crate) struct StreamGuard {
    created_at: Instant,
    initial_upstream_bytes: u64,
    destination: Option<protocol::Address>,
    reason: Option<io::Error>,
    stats: Option<CopyBidirectionalStats>,
}

impl Default for StreamGuard {
    fn default() -> Self {
        Self {
            created_at: Instant::now(),
            initial_upstream_bytes: 0,
            destination: None,
            reason: None,
            stats: None,
        }
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        #[cfg(feature = "tracing")]
        {
            let (up, down) = self
                .stats
                .as_ref()
                .map(|s| (s.a_to_b_bytes, s.b_to_a_bytes))
                .unwrap_or_default();

            info!(
                dest = %self.destination.as_ref().map(|a| a.to_string()).unwrap_or_else(|| "unknown".to_string()),
                up = up + self.initial_upstream_bytes,
                down,
                duration = self.created_at.elapsed().as_millis(),
                reason = %DisconnectReason(&self.reason),
            );
        }
    }
}

pub(crate) struct DisconnectReason<'a>(pub(crate) &'a Option<io::Error>);

impl std::fmt::Display for DisconnectReason<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(e) => write!(f, "{}", e),
            None => write!(f, "ok"),
        }
    }
}
