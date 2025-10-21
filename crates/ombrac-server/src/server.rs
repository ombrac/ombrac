use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::broadcast;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac_macros::{error, info};
use ombrac_transport::{Acceptor, Connection};

use crate::connection::{ClientConnection, HandshakeValidator};

pub struct Server<T, V> {
    acceptor: Arc<T>,
    validator: Arc<V>,
}

impl<T: Acceptor, V: HandshakeValidator + 'static> Server<T, V> {
    pub fn new(acceptor: T, validator: V) -> Self {
        Self {
            acceptor: Arc::new(acceptor),
            validator: Arc::new(validator),
        }
    }

    pub async fn accept_loop(&self, mut shutdown_rx: broadcast::Receiver<()>) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                accepted = self.acceptor.accept() => {
                    match accepted {
                        Ok(connection) => {
                            let validator = Arc::clone(&self.validator);
                            #[cfg(not(feature = "tracing"))]
                            tokio::spawn(Self::handle_connection(connection, validator));
                            #[cfg(feature = "tracing")]
                            tokio::spawn(Self::handle_connection(connection, validator).in_current_span());
                        },
                        Err(_err) => error!("failed to accept connection: {}", _err)
                    }
                },
            }
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing",
        tracing::instrument(
            name = "connection",
            skip_all,
            fields(
                id = connection.id(),
                from = tracing::field::Empty,
                secret = tracing::field::Empty
            )
        )
    )]
    pub async fn handle_connection(connection: <T as Acceptor>::Connection, validator: Arc<V>) {
        #[cfg(feature = "tracing")]
        let created_at = Instant::now();

        let peer_addr = match connection.remote_address() {
            Ok(addr) => addr,
            Err(_err) => {
                return error!("failed to get remote address for incoming connection {_err}");
            }
        };

        #[cfg(feature = "tracing")]
        tracing::Span::current().record("from", tracing::field::display(peer_addr));

        let reason: std::borrow::Cow<'static, str> = {
            match ClientConnection::handle(connection, validator.as_ref()).await {
                Ok(_) => "ok".into(),
                Err(e) => {
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                    ) {
                        format!("client disconnect: {}", e.kind()).into()
                    } else {
                        error!("connection handler failed: {e}");
                        format!("error: {e}").into()
                    }
                }
            }
        };

        info!(
            duration = created_at.elapsed().as_millis(),
            reason = %reason.as_ref(),
            "connection closed"
        );
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.acceptor.local_addr()
    }
}
