use ombrac_protocol::Provider;

pub struct Stream<T> {
    inner: T,
}

pub struct Builder<T> {
    connection: T,
}

mod s2n_quic {
    use s2n_quic::stream::BidirectionalStream;
    use s2n_quic::Connection as NoiseConnection;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    use super::{Builder, Provider, Stream};

    impl<T> Builder<T>
    where
        T: Provider<NoiseConnection> + Send + 'static,
    {
        pub fn new(connection: T) -> Self {
            Self { connection }
        }

        pub fn build(self) -> impl Provider<BidirectionalStream> {
            let (stream_sender, stream_receiver) = mpsc::channel(1);
            let mut connection = self.connection;

            tokio::spawn(async move {
                while let Some(mut connection) = connection.fetch().await {
                    let stream_sender = stream_sender.clone();

                    tokio::spawn(async move {
                        while let Ok(Some(stream)) = connection.accept_bidirectional_stream().await
                        {
                            if stream_sender.send(stream).await.is_err() {
                                break;
                            }
                        }
                    });
                }
            });

            Stream {
                inner: stream_receiver,
            }
        }
    }

    impl Provider<BidirectionalStream> for Stream<Receiver<BidirectionalStream>> {
        async fn fetch(&mut self) -> Option<BidirectionalStream> {
            self.inner.recv().await
        }
    }
}
