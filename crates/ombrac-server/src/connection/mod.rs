#[cfg(feature = "datagram")]
mod datagram;
mod stream;

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::Weak;

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

pub struct ConnectionHandle<C> {
    inner: Arc<C>,
}

impl<C: Connection> ConnectionHandle<C> {
    pub fn downgrade_inner(&self) -> Weak<C> {
        Arc::downgrade(&self.inner)
    }

    pub fn close(&self, error_code: u32, reason: &[u8]) {
        self.inner.close(error_code, reason);
    }
}

pub trait ConnectionHandler<T>: Send + Sync {
    type Context: Send;

    fn verify(
        &self,
        hello: &protocol::ClientHello,
    ) -> impl Future<Output = Result<Self::Context, protocol::HandshakeError>> + Send;

    fn accept(
        &self,
        output: Self::Context,
        connection: ConnectionHandle<T>,
    ) -> impl Future<Output = ()> + Send;
}

impl<T: Send + Sync> ConnectionHandler<T> for ombrac::protocol::Secret {
    type Context = ();

    async fn verify(&self, hello: &protocol::ClientHello) -> Result<(), protocol::HandshakeError> {
        if &hello.secret == self {
            Ok(())
        } else {
            Err(protocol::HandshakeError::InvalidSecret)
        }
    }

    async fn accept(&self, _output: Self::Context, _connection: ConnectionHandle<T>) {
        ()
    }
}

pub struct ConnectionDriver<C: Connection> {
    client_connection: Arc<C>,
    shutdown_token: CancellationToken,
}

impl<C: Connection> ConnectionDriver<C> {
    pub async fn handle<V>(connection: C, validator: &V) -> io::Result<()>
    where
        V: ConnectionHandler<C>,
    {
        let (validation_ctx, connection) = Self::perform_handshake(connection, validator).await?;

        let client_connection = Arc::new(connection);

        validator
            .accept(
                validation_ctx,
                ConnectionHandle {
                    inner: client_connection.clone(),
                },
            )
            .await;

        let handler = Self {
            client_connection,
            shutdown_token: CancellationToken::new(),
        };

        handler.run_acceptor_loops().await;

        Ok(())
    }

    async fn perform_handshake<V>(connection: C, validator: &V) -> io::Result<(V::Context, C)>
    where
        V: ConnectionHandler<C>,
    {
        let mut control_stream = connection.accept_bidirectional().await?;
        let mut control_frame = Framed::new(&mut control_stream, codec::length_codec());

        let payload = match control_frame.next().await {
            Some(Ok(bytes)) => bytes,
            Some(Err(e)) => return Err(e),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Stream closed before hello",
                ));
            }
        };

        let message: codec::UpstreamMessage = protocol::decode(&payload)?;

        let hello = match message {
            codec::UpstreamMessage::Hello(h) => h,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Expected Hello message",
                ));
            }
        };

        #[cfg(feature = "tracing")]
        Self::trace_handshake(&hello);

        let validation_result = if hello.version != protocol::PROTOCOLS_VERSION {
            Err(protocol::HandshakeError::UnsupportedVersion)
        } else {
            validator.verify(&hello).await
        };

        let response = match validation_result {
            Ok(_) => protocol::ServerHandshakeResponse::Ok,
            Err(ref e) => protocol::ServerHandshakeResponse::Err(e.clone()),
        };

        control_frame.send(protocol::encode(&response)?).await?;

        match validation_result {
            Ok(ctx) => Ok((ctx, connection)),
            Err(e) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Handshake failed: {:?}", e),
            )),
        }
    }

    async fn run_acceptor_loops(&self) {
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
            Ok(Ok(_)) => debug!("Connection closed gracefully."),
            Ok(Err(e)) => debug!("Connection closed with internal error: {}", e),
            Err(e) => warn!("Connection handler task panicked or failed: {}", e),
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

    #[cfg(feature = "tracing")]
    fn trace_handshake(hello: &protocol::ClientHello) {
        let secret_hex = hello
            .secret
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        tracing::Span::current().record("secret", &secret_hex);
    }
}
