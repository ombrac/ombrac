mod datagram;
mod stream;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use moka::future::Cache;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, warn};

use ombrac::codec::{LengthDelimitedCodec, ServerHandshakeResponse, UpstreamMessage, length_codec};
use ombrac::protocol::{self, Address, HandshakeError, PROTOCOLS_VERSION, Secret, UdpPacket};
use ombrac_transport::Connection;

pub struct ConnectionHandler<C: Connection> {
    connection: Arc<C>,
    cancellation_token: CancellationToken,
}

impl<C: Connection> ConnectionHandler<C> {
    pub async fn handle(connection: C, secret: Secret) -> io::Result<()> {
        let mut control_stream = connection.accept_bidirectional().await?;
        let mut framed_control = Framed::new(&mut control_stream, length_codec());

        match framed_control.next().await {
            Some(Ok(payload)) => {
                let hello_message: UpstreamMessage = protocol::decode(&payload)?;
                Self::validate_handshake(hello_message, secret, &mut framed_control).await?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Failed to read Hello message",
                ));
            }
        }

        let handler = Self {
            connection: Arc::new(connection),
            cancellation_token: CancellationToken::new(),
        };

        handler.run_proxy_tasks().await;

        Ok(())
    }

    /// Validates the client's Hello message and sends the appropriate response.
    async fn validate_handshake(
        message: UpstreamMessage,
        secret: Secret,
        framed: &mut Framed<&mut C::Stream, LengthDelimitedCodec>,
    ) -> io::Result<()> {
        if let UpstreamMessage::Hello(hello) = message {
            let response = if hello.version != PROTOCOLS_VERSION {
                warn!("Handshake failed: Unsupported protocol version",);
                ServerHandshakeResponse::Err(HandshakeError::UnsupportedVersion)
            } else if hello.secret != secret {
                warn!("Handshake failed: Invalid secret");
                ServerHandshakeResponse::Err(HandshakeError::InvalidSecret)
            } else {
                ServerHandshakeResponse::Ok
            };

            let response_bytes = protocol::encode(&response)?;
            framed.send(response_bytes).await?;

            if matches!(response, ServerHandshakeResponse::Err(_)) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Handshake failed",
                ));
            }
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected Hello message",
            ))
        }
    }

    /// Spawns and manages the long-running tasks for TCP and UDP proxying.
    ///
    /// This function waits for either the TCP or UDP handler to exit (due to an
    /// error or graceful shutdown) and then triggers a cancellation for all other
    /// tasks associated with this connection.
    async fn run_proxy_tasks(&self) {
        let tcp_handler = self.spawn_tcp_handler();

        #[cfg(feature = "datagram")]
        let udp_handler = self.spawn_udp_handler();

        // Wait for either handler to complete or fail.
        #[cfg(feature = "datagram")]
        let result = tokio::select! {
            res = tcp_handler => res,
            res = udp_handler => res,
        };

        // If datagram feature is disabled, only await the TCP handler.
        #[cfg(not(feature = "datagram"))]
        let result = tcp_handler.await;

        self.cancellation_token.cancel(); // Signal all related tasks to shut down.

        match result {
            Ok(Ok(_)) => {
                debug!("Client connection closed gracefully.");
            }
            Ok(Err(e)) => {
                if e.kind() != io::ErrorKind::ConnectionAborted {
                    warn!("Client connection closed with an error: {e}",);
                }
            }
            Err(_join_err) => {
                warn!("Client connection handler task failed: {_join_err}",);
            }
        }
    }

    /// Spawns the task responsible for accepting and handling new TCP streams.
    fn spawn_tcp_handler(&self) -> JoinHandle<io::Result<()>> {
        let connection = Arc::clone(&self.connection);
        let token = self.cancellation_token.child_token();

        tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return Ok(()),
                        result = connection.accept_bidirectional() => {
                            let stream = result?;

                            // Spawn a separate task for each TCP stream to avoid blocking the acceptor.

                            tokio::spawn(async move {
                                if let Err(_e) = Self::handle_tcp_stream(stream).await {
                                    warn!(error = %_e, "Stream handler error");
                                }
                            }.in_current_span());
                        }
                    }
                }
            }
            .in_current_span(),
        )
    }

    /// Handles a single TCP stream, proxying data to the requested destination.
    async fn handle_tcp_stream(mut stream: C::Stream) -> io::Result<()> {
        let start_time = Instant::now();
        let mut framed = Framed::new(&mut stream, length_codec());

        // Read the destination address from the client.
        let original_dest = match framed.next().await {
            Some(Ok(payload)) => match protocol::decode(&payload)? {
                UpstreamMessage::Connect(connect) => connect.address,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Expected Connect message",
                    ));
                }
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Failed to read Connect message on new stream",
                ));
            }
        };

        let mut dest_stream = TcpStream::connect(original_dest.to_socket_addr().await?).await?;

        // Forward any data that was already buffered in the framing codec.
        let parts = framed.into_parts();
        let mut stream = parts.io;

        let initial_sent_bytes = parts.read_buf.len() as u64;

        if !parts.read_buf.is_empty() {
            dest_stream.write_all(&parts.read_buf).await?;
        }

        // Copy data in both directions until one side closes.
        let copy_result =
            ombrac_transport::io::copy_bidirectional(&mut stream, &mut dest_stream).await;

        let (mut sent, received) = match &copy_result {
            Ok(stats) => (stats.a_to_b_bytes, stats.b_to_a_bytes),
            Err((_, stats)) => (stats.a_to_b_bytes, stats.b_to_a_bytes),
        };

        sent += initial_sent_bytes;

        tracing::info!(
            dest = %original_dest,
            upstream_bytes = sent,
            upstream_bytes = received,
            duration_ms = start_time.elapsed().as_millis()
        );

        copy_result.map(|_| ()).map_err(|(e, _)| e)
    }

    #[cfg(feature = "datagram")]
    fn spawn_udp_handler(&self) -> JoinHandle<io::Result<()>> {
        use crate::connection::datagram::DatagramContext;

        let context = DatagramContext::new(
            Arc::clone(&self.connection),
            self.cancellation_token.child_token(),
        );
        tokio::spawn(async move { context.run_associate_loop().await }.in_current_span())
    }
}
