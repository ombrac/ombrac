#[cfg(feature = "datagram")]
mod datagram;
mod stream;

use std::io;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac::codec;
use ombrac::protocol;
use ombrac_macros::{debug, warn};
use ombrac_transport::Connection;

pub trait HandshakeValidator: Send + Sync {
    fn validate_hello(&self, hello: &protocol::ClientHello)
    -> Result<(), protocol::HandshakeError>;
}

pub struct ClientConnection<C: Connection> {
    client_connection: Arc<C>,
    shutdown_token: CancellationToken,
}

impl<C: Connection> ClientConnection<C> {
    pub async fn handle<V: HandshakeValidator>(connection: C, validator: &V) -> io::Result<()> {
        let mut control_stream = connection.accept_bidirectional().await?;
        let mut control_frame = Framed::new(&mut control_stream, codec::length_codec());

        match control_frame.next().await {
            Some(Ok(payload)) => {
                let hello_message: codec::UpstreamMessage = protocol::decode(&payload)?;

                if let codec::UpstreamMessage::Hello(hello) = &hello_message {
                    #[cfg(feature = "tracing")]
                    {
                        let secret_hex = hello
                            .secret
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<String>();
                        tracing::span::Span::current().record("secret", &secret_hex);
                    }

                    let response = if hello.version != protocol::PROTOCOLS_VERSION {
                        protocol::ServerHandshakeResponse::Err(
                            protocol::HandshakeError::UnsupportedVersion,
                        )
                    } else {
                        match validator.validate_hello(hello) {
                            Ok(_) => protocol::ServerHandshakeResponse::Ok,
                            Err(e) => protocol::ServerHandshakeResponse::Err(e),
                        }
                    };

                    let response_payload = protocol::encode(&response)?;
                    control_frame.send(response_payload.into()).await?;

                    if let protocol::ServerHandshakeResponse::Err(e) = response {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!("handshake validation failed: {:?}", e),
                        ));
                    }
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "failed to read hello message",
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
                debug!("connection closed with an error: {_err}");
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

        #[cfg(not(feature = "tracing"))]
        let handle = tokio::spawn(tunnel.accept_loop());
        #[cfg(feature = "tracing")]
        let handle = tokio::spawn(tunnel.accept_loop().in_current_span());

        handle
    }

    #[cfg(feature = "datagram")]
    fn spawn_client_datagram_acceptor(&self) -> JoinHandle<io::Result<()>> {
        use crate::connection::datagram::DatagramTunnel;

        let connection = Arc::clone(&self.client_connection);
        let shutdown = self.shutdown_token.child_token();
        let tunnel = DatagramTunnel::new(connection, shutdown);

        #[cfg(not(feature = "tracing"))]
        let handle = tokio::spawn(tunnel.accept_loop());
        #[cfg(feature = "tracing")]
        let handle = tokio::spawn(tunnel.accept_loop().in_current_span());

        handle
    }
}
