use std::io;
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use ombrac::{codec, protocol};
use ombrac_transport::Connection;
use ombrac_transport::io::{CopyBidirectionalStats, copy_bidirectional};

pub(crate) struct StreamGuard {
    start_time: Instant,
    initial_upstream_bytes: u64,
    destination: Option<protocol::Address>,
    reason: Option<io::Error>,
    stats: Option<CopyBidirectionalStats>,
}

impl Default for StreamGuard {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            initial_upstream_bytes: 0,
            destination: None,
            reason: None,
            stats: None,
        }
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        let (upstream_bytes, downstream_bytes) = self
            .stats
            .as_ref()
            .map(|s| (s.a_to_b_bytes, s.b_to_a_bytes))
            .unwrap_or_default();

        tracing::info!(
            destination = %self.destination.as_ref().map(|a| a.to_string()).unwrap_or_else(|| "unknown".to_string()),
            upstream_bytes = upstream_bytes + self.initial_upstream_bytes,
            downstream_bytes,
            duration_ms = self.start_time.elapsed().as_millis(),
            reason = %DisconnectReason(&self.reason),
        );
    }
}

pub(crate) struct StreamTunnel<C: Connection> {
    connection: Arc<C>,
    shutdown: CancellationToken,
}

impl<C: Connection> StreamTunnel<C> {
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
                    tokio::spawn(async move {
                        let mut guard = StreamGuard::default();
                        if let Err(e) = Self::handle_connect(stream, &mut guard).await {
                            guard.reason = Some(e);
                        }
                    }.in_current_span());
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
        guard.destination = Some(destination.clone());

        // TODO: use hickory-dns
        let mut tcp_stream = TcpStream::connect(destination.to_socket_addr().await?).await?;

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
                Err(e.into())
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
            Some(Err(e)) => Err(e.into()),
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "failed to read connect message on stream",
            )),
        }
    }
}

struct DisconnectReason<'a>(&'a Option<io::Error>);

impl std::fmt::Display for DisconnectReason<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(e) => write!(f, "{}", e),
            None => write!(f, "ok"),
        }
    }
}
