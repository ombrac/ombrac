use tokio::sync::mpsc::{self, Receiver};

use crate::{debug, error};

pub mod impl_s2n_quic {
    use s2n_quic::stream::BidirectionalStream as NoiseStream;
    use s2n_quic::Connection as NoiseConnection;

    use super::*;

    pub async fn stream(
        mut connection: Receiver<NoiseConnection>,
        max_multiplex: u64,
    ) -> Receiver<NoiseStream> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            'connection: while let Some(mut connection) = connection.recv().await {
                let mut multiplex = 1;

                'stream: loop {
                    let stream = match connection.open_bidirectional_stream().await {
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
                        "{:?} connection {} open bidirectional stream {}",
                        connection.local_addr(),
                        connection.id(),
                        stream.id()
                    );

                    if sender.send(stream).await.is_err() {
                        break 'connection;
                    }

                    if max_multiplex != 0 {
                        if multiplex >= max_multiplex {
                            break 'stream;
                        }
                        multiplex = multiplex.wrapping_add(1);
                    }
                }
            }
        });

        receiver
    }
}
