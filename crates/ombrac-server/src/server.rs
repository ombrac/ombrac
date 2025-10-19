use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use ombrac::protocol::Secret;
use tokio::sync::broadcast;
use tracing::{Instrument, error, info};

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
                            let secret = self.secret;
                            let peer_addr = connection.remote_address().unwrap();
                            let connection_id = connection.id();

                            let conn_span = tracing::info_span!("connection", id = connection_id, from = %peer_addr, secret = tracing::field::Empty);
                            tokio::spawn(async move {
                                let start_time = Instant::now();
                                if let Err(e) = ClientConnection::handle(connection, secret).await {
                                    if !matches!(e.kind(),
                                        io::ErrorKind::ConnectionReset |
                                        io::ErrorKind::BrokenPipe |
                                        io::ErrorKind::UnexpectedEof
                                    ) {
                                        error!("Connection handler failed, {e}");
                                    }
                                }
                                info!(duration_ms = start_time.elapsed().as_millis(), "connection closed");
                            }.instrument(conn_span));
                        },
                        Err(_err) => {
                            error!("failed to accept connection: {}", _err)
                        },
                    }
                },
            }
        }

        Ok(())
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.acceptor.local_addr()
    }
}
