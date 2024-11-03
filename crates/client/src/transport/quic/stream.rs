use tokio::sync::mpsc::{self, Receiver};

use crate::{debug, error};

pub mod impl_s2n_quic {
    use std::time::Duration;

    use s2n_quic::stream::BidirectionalStream as NoiseStream;
    use s2n_quic::Connection as NoiseConnection;
    use tokio::time::interval;

    use super::*;

    pub async fn stream(
        mut connection: Receiver<NoiseConnection>,
        max_multiplex: u64,
        max_multiplex_interval: (u64, Duration),
    ) -> Receiver<NoiseStream> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            'connection: while let Some(mut connection) = connection.recv().await {
                let mut multiplex = 1;

                let mut interval_timer = interval(max_multiplex_interval.1);
                let mut interval_multiplex = 0;

                'stream: loop {
                    tokio::select! {
                        _ = interval_timer.tick() => {
                            interval_multiplex = 0;
                        }

                        result = connection.open_bidirectional_stream() => {
                            let stream = match result {
                                Ok(stream) => stream,
                                Err(_error) => {
                                    error!(
                                        "connection {} failed to open bidirectional stream. {}",
                                        connection.id(),
                                        _error
                                    );
                                    break 'stream;
                                }
                            };

                            debug!(
                                "{:?} connection {} open bidirectional stream {}.",
                                connection.local_addr(),
                                connection.id(),
                                stream.id()
                            );

                            if sender.send(stream).await.is_err() {
                                break 'connection;
                            }

                            interval_multiplex += 1;
                            if interval_multiplex >= max_multiplex_interval.0 {
                                debug!(
                                    "connection {} reached max multiplex limit {} in {}s, switching connection.",
                                    connection.id(),
                                    max_multiplex_interval.0,
                                    max_multiplex_interval.1.as_secs()
                                );
                                break 'stream;
                            }

                            if max_multiplex != 0 {
                                if multiplex >= max_multiplex {
                                    debug!(
                                        "connection {} reached max multiplex limit {}, switching connection.",
                                        connection.id(),
                                        max_multiplex
                                    );
                                    break 'stream;
                                }
                                multiplex = multiplex.wrapping_add(1);
                            }
                        }
                    }
                }
            }
        });

        receiver
    }
}
