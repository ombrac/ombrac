mod datagram;
mod stream;

use std::io;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, warn};

use ombrac::codec::{LengthDelimitedCodec, ServerHandshakeResponse, UpstreamMessage, length_codec};
use ombrac::protocol::{self, HandshakeError, PROTOCOLS_VERSION, Secret};
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

        handler.manage_acceptor_loops().await;

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
                warn!("handshake failed: unsupported protocol version");
                ServerHandshakeResponse::Err(HandshakeError::UnsupportedVersion)
            }
            UpstreamMessage::Hello(hello) if hello.secret != secret => {
                warn!("handshake failed: invalid secret");
                ServerHandshakeResponse::Err(HandshakeError::InvalidSecret)
            }
            UpstreamMessage::Hello(_) => ServerHandshakeResponse::Ok,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected Hello message",
                ));
            }
        };

        let response_bytes = protocol::encode(&response)?;
        framed.send(response_bytes).await?;

        if matches!(response, ServerHandshakeResponse::Err(_)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "handshake failed",
            ));
        }

        Ok(())
    }

    async fn manage_acceptor_loops(&self) {
        let connect_acceptor = self.spawn_client_connect_acceptor();
        #[cfg(feature = "datagram")]
        let datagram_acceptor = self.spawn_client_datagram_acceptor();

        #[cfg(not(feature = "datagram"))]
        let result = connect_acceptor.await;

        #[cfg(feature = "datagram")]
        let result = tokio::select! {
            res = connect_acceptor => res,
            res = datagram_acceptor => res,
        };

        // Signal all related tasks to shut down
        self.shutdown_token.cancel();

        match result {
            Ok(Ok(_)) => {
                debug!("connection closed gracefully.");
            }
            Ok(Err(_err)) => {
                warn!("connection closed with an error: {_err}");
            }
            Err(_err) => {
                warn!("connection handler task failed: {_err}");
            }
        }
    }

    fn spawn_client_connect_acceptor(&self) -> JoinHandle<io::Result<()>> {
        use crate::connection::stream::StreamTunnel;

        let connection = Arc::clone(&self.client_connection);
        let shutdown = self.shutdown_token.child_token();
        let tunnel = StreamTunnel::new(connection, shutdown);

        tokio::spawn(tunnel.accept_loop().in_current_span())
    }

    #[cfg(feature = "datagram")]
    fn spawn_client_datagram_acceptor(&self) -> JoinHandle<io::Result<()>> {
        use crate::connection::datagram::DatagramTunnel;

        let connection = Arc::clone(&self.client_connection);
        let shutdown = self.shutdown_token.child_token();
        let tunnel = DatagramTunnel::new(connection, shutdown);

        tokio::spawn(tunnel.accept_loop().in_current_span())
    }
}
