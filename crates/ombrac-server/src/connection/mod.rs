mod datagram;
mod stream;

use std::io;
use std::sync::Arc;
use std::time::Instant;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, warn};

use ombrac::codec::{LengthDelimitedCodec, ServerHandshakeResponse, UpstreamMessage, length_codec};
use ombrac::protocol::{self, Address, HandshakeError, PROTOCOLS_VERSION, Secret, UdpPacket};
use ombrac_transport::Connection;

pub struct ClientConnection<C: Connection> {
    client_connection: Arc<C>,
    shutdown_token: CancellationToken,
}

impl<C: Connection> ClientConnection<C> {
    pub async fn handle(connection: C, secret: Secret) -> io::Result<()> {
        let mut control_stream = connection.accept_bidirectional().await?;
        let mut control_frame = Framed::new(&mut control_stream, length_codec());

        match control_frame.next().await {
            Some(Ok(payload)) => {
                let hello_message: UpstreamMessage = protocol::decode(&payload)?;
                Self::validate_handshake(hello_message, secret, &mut control_frame).await?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Failed to read Hello message",
                ));
            }
        }

        let handler = Self {
            client_connection: Arc::new(connection),
            shutdown_token: CancellationToken::new(),
        };

        handler.manage_proxy_loops().await;

        Ok(())
    }

    /// Validates the client's Hello message and sends the appropriate response.
    async fn validate_handshake(
        message: UpstreamMessage,
        secret: Secret,
        framed: &mut Framed<&mut C::Stream, LengthDelimitedCodec>,
    ) -> io::Result<()> {
        let response = match message {
            UpstreamMessage::Hello(hello) if hello.version != PROTOCOLS_VERSION => {
                warn!("Handshake failed: Unsupported protocol version");
                ServerHandshakeResponse::Err(HandshakeError::UnsupportedVersion)
            }
            UpstreamMessage::Hello(hello) if hello.secret != secret => {
                warn!("Handshake failed: Invalid secret");
                ServerHandshakeResponse::Err(HandshakeError::InvalidSecret)
            }
            UpstreamMessage::Hello(_) => ServerHandshakeResponse::Ok,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Expected Hello message",
                ));
            }
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
    }

    /// Spawns and manages the long-running tasks for TCP and UDP proxying.
    ///
    /// This function waits for either the TCP or UDP handler to exit (due to an
    /// error or graceful shutdown) and then triggers a cancellation for all other
    /// tasks associated with this connection.
    async fn manage_proxy_loops(&self) {
        let tcp_acceptor = self.spawn_tcp_acceptor();

        #[cfg(feature = "datagram")]
        let udp_acceptor = self.spawn_udp_acceptor();

        #[cfg(not(feature = "datagram"))]
        let result = tcp_acceptor.await;

        #[cfg(feature = "datagram")]
        let result = tokio::select! {
            res = tcp_acceptor => res,
            res = udp_acceptor => res,
        };

        self.shutdown_token.cancel(); // Signal all related tasks to shut down.

        match result {
            Ok(Ok(_)) => {
                debug!("Client connection closed gracefully.");
            }
            Ok(Err(e)) => {
                if e.kind() != io::ErrorKind::ConnectionAborted {
                    warn!("Client connection closed with an error: {e}");
                }
            }
            Err(join_err) => {
                warn!("Client connection handler task failed: {join_err}");
            }
        }
    }

    /// Spawns the task responsible for accepting and handling new TCP streams.
    fn spawn_tcp_acceptor(&self) -> JoinHandle<io::Result<()>> {
        let connection = Arc::clone(&self.client_connection);
        let token = self.shutdown_token.child_token();

        tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return Ok(()),
                        result = connection.accept_bidirectional() => {
                            let stream = result?;

                            // Spawn a separate task for each TCP stream to avoid blocking the acceptor.
                            tokio::spawn(async move {
                                if let Err(e) = Self::proxy_tcp_stream(stream).await {
                                    warn!(error = %e, "Stream handler error");
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
    async fn proxy_tcp_stream(mut stream: C::Stream) -> io::Result<()> {
        let start_time = Instant::now();
        let mut framed = Framed::new(&mut stream, length_codec());

        let target_addr = Self::receive_connect_message(&mut framed).await?;
        let mut target_stream = TcpStream::connect(target_addr.to_socket_addr().await?).await?;

        // Forward any data that was already buffered in the framing codec.
        let parts = framed.into_parts();
        let mut stream = parts.io;

        let initial_sent_bytes = parts.read_buf.len() as u64;
        if !parts.read_buf.is_empty() {
            target_stream.write_all(&parts.read_buf).await?;
        }

        // Copy data in both directions until one side closes.
        let copy_result =
            ombrac_transport::io::copy_bidirectional(&mut stream, &mut target_stream).await;

        let (mut sent, received) = match &copy_result {
            Ok(stats) => (stats.a_to_b_bytes, stats.b_to_a_bytes),
            Err((_, stats)) => (stats.a_to_b_bytes, stats.b_to_a_bytes),
        };
        sent += initial_sent_bytes;

        info!(
            dest = %target_addr,
            upstream_bytes = sent,
            downstream_bytes = received,
            duration_ms = start_time.elapsed().as_millis()
        );

        copy_result.map(|_| ()).map_err(|(e, _)| e)
    }

    async fn receive_connect_message(
        framed: &mut Framed<&mut C::Stream, LengthDelimitedCodec>,
    ) -> io::Result<Address> {
        match framed.next().await {
            Some(Ok(payload)) => match protocol::decode(&payload)? {
                UpstreamMessage::Connect(connect) => Ok(connect.address),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Expected Connect message",
                )),
            },
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Failed to read Connect message on new stream",
            )),
        }
    }

    #[cfg(feature = "datagram")]
    fn spawn_udp_acceptor(&self) -> JoinHandle<io::Result<()>> {
        let context = crate::connection::datagram::Datagram::new(
            Arc::clone(&self.client_connection),
            self.shutdown_token.child_token(),
        );
        tokio::spawn(async move { context.run_upstream_loop().await }.in_current_span())
    }
}
