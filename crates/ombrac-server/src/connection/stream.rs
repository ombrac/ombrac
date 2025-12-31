use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac::{codec, protocol};
use ombrac_macros::info;
use ombrac_transport::Connection;
use ombrac_transport::io::{CopyBidirectionalStats, copy_bidirectional};

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

pub(crate) struct StreamTunnel<C: Connection> {
    connection: Arc<C>,
    shutdown: CancellationToken,
}

impl<C: Connection> StreamTunnel<C> {
    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

    pub(crate) fn new(connection: Arc<C>, shutdown: CancellationToken) -> Self {
        Self {
            connection,
            shutdown,
        }
    }

    pub(crate) async fn accept_loop(self) -> io::Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => break,
                result = self.connection.accept_bidirectional() => {
                    let stream = result?;
                    let future = async move {
                        let mut guard = StreamGuard::default();
                        if let Err(e) = Self::handle_connect(stream, &mut guard).await {
                            guard.reason = Some(e);
                        }
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
    ) -> io::Result<()> {
        let mut framed = Framed::new(&mut stream, codec::length_codec());

        // Step 1: Read the connection request from the client (with timeout)
        let destination = Self::read_connect_message(&mut framed).await?;
        guard.destination = Some(destination.clone());

        // Step 2: Attempt to connect to the destination (with timeout)
        let connect_result = Self::connect_to_destination(&destination).await;

        // Step 3: Send connection response to client
        Self::send_connect_response(&mut framed, &connect_result).await?;

        // Step 4: If connection failed, return error
        let mut tcp_stream = connect_result?;

        // Step 5: Exchange data between client and destination
        // Note: This phase has no timeout as it's the normal data transfer phase
        // that can last for the entire lifetime of the TCP connection.
        Self::exchange_data(framed, &mut tcp_stream, guard).await
    }

    /// Reads the connect message from the client.
    ///
    /// This function includes a timeout to prevent hanging on unresponsive clients.
    async fn read_connect_message(
        framed: &mut Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
    ) -> io::Result<protocol::Address> {
        match tokio::time::timeout(Self::DEFAULT_CONNECT_TIMEOUT, framed.next()).await {
            Ok(Some(Ok(payload))) => {
                let message = protocol::decode(&payload)?;
                match message {
                    codec::UpstreamMessage::Connect(connect) => Ok(connect.address),
                    _ => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Expected connect message",
                    )),
                }
            }
            Ok(Some(Err(e))) => Err(e),
            Ok(None) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Failed to read connect message on stream",
            )),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Timeout reading connect message",
            )),
        }
    }

    /// Attempts to connect to the destination address with a timeout.
    async fn connect_to_destination(destination: &protocol::Address) -> io::Result<TcpStream> {
        let destination_socket_addr = destination.to_socket_addr().await?;

        match tokio::time::timeout(
            Self::DEFAULT_CONNECT_TIMEOUT,
            TcpStream::connect(destination_socket_addr),
        )
        .await
        {
            Ok(Ok(stream)) => Ok(stream),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Connection timeout",
            )),
        }
    }

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

        let response_message = codec::DownstreamMessage::ConnectResponse(response);
        let encoded_response = protocol::encode(&response_message)?;
        framed.send(encoded_response).await?;

        Ok(())
    }

    /// Exchanges data between the client stream and the destination TCP stream.
    async fn exchange_data(
        framed: Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
        tcp_stream: &mut TcpStream,
        guard: &mut StreamGuard,
    ) -> io::Result<()> {
        // Extract any buffered data from the framed stream
        let parts = framed.into_parts();
        guard.initial_upstream_bytes = parts.read_buf.len() as u64;

        // Write any buffered upstream data to the destination
        if !parts.read_buf.is_empty() {
            tcp_stream.write_all(&parts.read_buf).await?;
        }

        // Perform bidirectional data copying
        let mut stream_inner = parts.io;
        match copy_bidirectional(&mut stream_inner, tcp_stream).await {
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

pub(crate) struct DisconnectReason<'a>(pub(crate) &'a Option<io::Error>);

impl std::fmt::Display for DisconnectReason<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(e) => write!(f, "{}", e),
            None => write!(f, "ok"),
        }
    }
}
