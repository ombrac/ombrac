use tokio::sync::mpsc::{self, Receiver};

use crate::{debug, error};

pub mod impl_s2n_quic {
    use s2n_quic::stream::BidirectionalStream as NoiseStream;
    use s2n_quic::Connection as NoiseConnection;

    use super::*;

    pub async fn stream(mut connection: Receiver<NoiseConnection>) -> Receiver<NoiseStream> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            'connection: loop {
                if let Some(mut connection) = connection.recv().await {
                    let stream = match connection.open_bidirectional_stream().await {
                        Ok(stream) => stream,
                        Err(_error) => {
                            error!(
                                "connection {} failed to open bidirectional stream. {}",
                                connection.id(),
                                _error
                            );

                            continue;
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
                }
            }
        });

        receiver
    }
}
