use std::io;
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

use ombrac::{codec, protocol};
use ombrac_transport::Connection;
use ombrac_transport::io::copy_bidirectional;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

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
                        if let Err(e) = Self::handle_client_connect(stream).await {
                            // warn!(error = %e, "Stream handler error");
                        }
                    }.in_current_span());
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn handle_client_connect(mut stream: C::Stream) -> io::Result<()> {
        let start_time = Instant::now();
        let mut framed = Framed::new(&mut stream, codec::length_codec());

        let target_addr = Self::receive_connect_message(&mut framed).await?;
        let mut target_stream = TcpStream::connect(target_addr.to_socket_addr().await?).await?;

        let parts = framed.into_parts();
        let mut stream = parts.io;

        let initial_upstream_bytes = parts.read_buf.len() as u64;

        if !parts.read_buf.is_empty() {
            target_stream.write_all(&parts.read_buf).await?;
        }

        let copy = copy_bidirectional(&mut stream, &mut target_stream).await;

        let (mut upstream_bytes, downstream_bytes) = match &copy {
            Ok(stats) => (stats.a_to_b_bytes, stats.b_to_a_bytes),
            Err((_, stats)) => (stats.a_to_b_bytes, stats.b_to_a_bytes),
        };
        upstream_bytes += initial_upstream_bytes;

        #[cfg(feature = "tracing")]
        tracing::info!(
            dest = %target_addr,
            upstream_bytes,
            downstream_bytes,
            duration_ms = start_time.elapsed().as_millis()
        );

        copy.map(|_| ()).map_err(|(e, _)| e)
    }

    async fn receive_connect_message(
        framed: &mut Framed<&mut C::Stream, codec::LengthDelimitedCodec>,
    ) -> io::Result<protocol::Address> {
        match framed.next().await {
            Some(Ok(payload)) => match protocol::decode(&payload)? {
                codec::UpstreamMessage::Connect(connect) => Ok(connect.address),
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
}
