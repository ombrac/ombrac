use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use futures::SinkExt;
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
                _ = self.shutdown.cancelled() => break,
                result = self.connection.accept_bidirectional() => {
                    let stream = result?;
                    let future = async move {
                        let mut guard = StreamGuard::default();
                        if let Err(e) = Self::handle_connect(stream, &mut guard).await {
                            guard.reason = Some(e);
                        }
                    };

                    #[cfg(feature = "tracing")]
                    tokio::spawn(future.in_current_span());
                    #[cfg(not(feature = "tracing"))]
                    tokio::spawn(future);
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

        let destination = Self::handle_connect_message(&mut framed).await?;
        let destination_socket_addr = destination.to_socket_addr().await?;
        guard.destination = Some(destination.clone());

        // Attempt to connect to the destination
        let connect_result = match tokio::time::timeout(
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
        };

        // Send connection response to client
        let (response, tcp_stream_result) = match connect_result {
            Ok(stream) => {
                // Connection successful
                (protocol::ServerConnectResponse::Ok, Ok(stream))
            }
            Err(e) => {
                // Connection failed - send error response
                let error_kind = Self::classify_error(&e);
                let error_message = e.to_string();
                (
                    protocol::ServerConnectResponse::Err {
                        kind: error_kind,
                        message: error_message.clone(),
                    },
                    Err(e),
                )
            }
        };

        let response_message = codec::DownstreamMessage::ConnectResponse(response);
        let encoded_response = protocol::encode(&response_message)?;
        framed.send(encoded_response).await?;

        // If connection failed, return error
        let mut tcp_stream = match tcp_stream_result {
            Ok(stream) => stream,
            Err(e) => {
                guard.reason = Some(e);
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "Failed to connect to destination",
                ));
            }
        };

        // Connection successful - continue with data exchange
        let parts = framed.into_parts();
        guard.initial_upstream_bytes = parts.read_buf.len() as u64;

        if !parts.read_buf.is_empty() {
            tcp_stream.write_all(&parts.read_buf).await?;
        }

        let mut stream_inner = parts.io;
        match copy_bidirectional(&mut stream_inner, &mut tcp_stream).await {
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

    /// Classifies an IO error into a ConnectErrorKind for better error handling.
    fn classify_error(error: &io::Error) -> protocol::ConnectErrorKind {
        match error.kind() {
            io::ErrorKind::TimedOut => protocol::ConnectErrorKind::TimedOut,
            io::ErrorKind::ConnectionRefused => protocol::ConnectErrorKind::ConnectionRefused,
            io::ErrorKind::NetworkUnreachable => protocol::ConnectErrorKind::NetworkUnreachable,
            io::ErrorKind::HostUnreachable => protocol::ConnectErrorKind::HostUnreachable,
            io::ErrorKind::NotFound => {
                // DNS resolution failures often manifest as NotFound
                if error.to_string().contains("could not be resolved") {
                    protocol::ConnectErrorKind::DnsResolutionFailed
                } else {
                    protocol::ConnectErrorKind::Other
                }
            }
            _ => {
                // Check error message for DNS-related errors
                let msg = error.to_string().to_lowercase();
                if msg.contains("dns") || msg.contains("resolve") || msg.contains("lookup") {
                    protocol::ConnectErrorKind::DnsResolutionFailed
                } else {
                    protocol::ConnectErrorKind::Other
                }
            }
        }
    }

    async fn handle_connect_message(
        framed: &mut Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
    ) -> io::Result<protocol::Address> {
        match framed.next().await {
            Some(Ok(payload)) => {
                let message = protocol::decode(&payload)?;
                match message {
                    codec::UpstreamMessage::Connect(connect) => Ok(connect.address),
                    _ => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "expected connect message",
                    )),
                }
            }
            Some(Err(e)) => Err(e),
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "failed to read connect message on stream",
            )),
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
