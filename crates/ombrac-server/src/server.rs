use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use ombrac::protocol::Secret;
use tokio::sync::broadcast;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use ombrac_macros::{error, info};
use ombrac_transport::{Acceptor, Connection};

use crate::connection::ClientConnection;

pub struct Server<T: Acceptor> {
    acceptor: Arc<T>,
    secret: Secret,
}

impl<T: Acceptor> Server<T> {
    pub fn new(acceptor: T, secret: Secret) -> Self {
        Self {
            acceptor: Arc::new(acceptor),
            secret,
        }
    }

    pub async fn accept_loop(&self, mut shutdown_rx: broadcast::Receiver<()>) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                accepted = self.acceptor.accept() => {
                    match accepted {
                        Ok(connection) => {
                            #[cfg(not(feature = "tracing"))]
                            tokio::spawn(Self::handle_connection(connection, self.secret));
                            #[cfg(feature = "tracing")]
                            tokio::spawn(Self::handle_connection(connection, self.secret).in_current_span());
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
    pub async fn handle_connection(connection: <T as Acceptor>::Connection, secret: Secret) {
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

        let reason: std::borrow::Cow<'static, str> =
            match ClientConnection::handle(connection, secret).await {
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
