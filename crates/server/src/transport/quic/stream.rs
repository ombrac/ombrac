use tokio::sync::mpsc::{self, Receiver};

pub mod impl_s2n_quic {
    use s2n_quic::stream::BidirectionalStream as NoiseStream;
    use s2n_quic::Connection as NoiseConnection;

    use super::*;

    pub async fn stream(mut connection: Receiver<NoiseConnection>) -> Receiver<NoiseStream> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            while let Some(mut connection) = connection.recv().await {
                let sender = sender.clone();

                tokio::spawn(async move {
                    while let Ok(Some(stream)) = connection.accept_bidirectional_stream().await {
                        if sender.send(stream).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        receiver
    }
}
