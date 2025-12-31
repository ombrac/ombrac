use std::error::Error;
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
                biased;
                _ = self.shutdown.cancelled() => break,
                result = self.connection.accept_bidirectional() => {
                    let stream = result?;
                    let future = async move {
                        let mut guard = StreamGuard::default();
                        // handle_connect is responsible for setting guard.reason on all error paths
                        let _ = Self::handle_connect(stream, &mut guard).await;
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

        // Step 1: Read the connection request from the client
        let destination = match Self::read_connect_message(&mut framed).await {
            Ok(addr) => {
                guard.destination = Some(addr.clone());
                addr
            }
            Err(e) => {
                let err = io::Error::new(e.kind(), e.to_string());
                guard.reason = Some(err);
                return Err(e);
            }
        };

        // Step 2: Attempt to connect to the destination
        let connect_result = Self::connect_to_destination(&destination).await;

        // Step 3: Send connection response to client
        if let Err(e) = Self::send_connect_response(&mut framed, &connect_result).await {
            let err = io::Error::new(e.kind(), e.to_string());
            guard.reason = Some(err);
            return Err(e);
        }

        // Step 4: If connection failed, return error
        let mut tcp_stream = match connect_result {
            Ok(stream) => stream,
            Err(e) => {
                let error_msg = format!("Failed to connect to destination: {}", e);
                let err = io::Error::new(io::ErrorKind::ConnectionRefused, error_msg.clone());
                guard.reason = Some(io::Error::new(e.kind(), error_msg));
                return Err(err);
            }
        };

        // Step 5: Exchange data between client and destination
        match Self::exchange_data(framed, &mut tcp_stream, guard).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // Only set reason if not already set (exchange_data may have set it)
                if guard.reason.is_none() {
                    let err = io::Error::new(e.kind(), e.to_string());
                    guard.reason = Some(err);
                }
                Err(e)
            }
        }
    }

    /// Reads the connect message from the client.
    async fn read_connect_message(
        framed: &mut Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
    ) -> io::Result<protocol::Address> {
        match framed.next().await {
            Some(Ok(payload)) => {
                let message = protocol::decode(&payload)?;
                match message {
                    codec::UpstreamMessage::Connect(connect) => Ok(connect.address),
                    _ => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Expected connect message",
                    )),
                }
            }
            Some(Err(e)) => Err(e),
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Failed to read connect message on stream",
            )),
        }
    }

    /// Attempts to connect to the destination address with a timeout.
    async fn connect_to_destination(
        destination: &protocol::Address,
    ) -> io::Result<TcpStream> {
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
                let error_kind = Self::classify_error(e);
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

    /// Classifies an IO error into a ConnectErrorKind for better error handling.
    ///
    /// This function attempts to categorize connection errors by examining both
    /// the error kind and the error message/chain. DNS resolution failures are
    /// particularly tricky as they can manifest in different ways across platforms.
    fn classify_error(error: &io::Error) -> protocol::ConnectErrorKind {
        // First, check the primary error kind
        match error.kind() {
            io::ErrorKind::TimedOut => return protocol::ConnectErrorKind::TimedOut,
            io::ErrorKind::ConnectionRefused => return protocol::ConnectErrorKind::ConnectionRefused,
            io::ErrorKind::NetworkUnreachable => return protocol::ConnectErrorKind::NetworkUnreachable,
            io::ErrorKind::HostUnreachable => return protocol::ConnectErrorKind::HostUnreachable,
            io::ErrorKind::NotFound => {
                // NotFound can indicate DNS resolution failure, but we need to verify
                if Self::is_dns_error(error) {
                    return protocol::ConnectErrorKind::DnsResolutionFailed;
                }
                // Otherwise, NotFound might be a different issue
                return protocol::ConnectErrorKind::Other;
            }
            _ => {
                // For other error kinds, check if it's a DNS-related error
                if Self::is_dns_error(error) {
                    return protocol::ConnectErrorKind::DnsResolutionFailed;
                }
            }
        }

        protocol::ConnectErrorKind::Other
    }

    /// Checks if an error is related to DNS resolution failure.
    ///
    /// This function examines the error message and error chain to determine
    /// if the error is DNS-related. It uses multiple heuristics to be more robust
    /// across different platforms and error reporting styles.
    fn is_dns_error(error: &io::Error) -> bool {
        // Check the error message for DNS-related keywords
        let error_msg = error.to_string().to_lowercase();
        
        // Common DNS-related keywords across platforms
        let dns_keywords = [
            "dns",
            "resolve",
            "resolution",
            "lookup",
            "name resolution",
            "hostname",
            "nodename",
            "name or service not known",
            "could not be resolved",
            "no such host",
            "temporary failure",
            "name server",
        ];

        if dns_keywords.iter().any(|keyword| error_msg.contains(keyword)) {
            return true;
        }

        // Check the error source chain for DNS-related errors
        // This is more robust as it can catch wrapped DNS errors
        let mut source = error.source();
        while let Some(err) = source {
            let source_msg = format!("{}", err).to_lowercase();
            if dns_keywords.iter().any(|keyword| source_msg.contains(keyword)) {
                return true;
            }
            source = err.source();
        }

        false
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
