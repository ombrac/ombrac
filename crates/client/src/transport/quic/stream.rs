use ombrac_protocol::Provider;

pub struct Stream<T> {
    inner: T,
}

pub struct Builder<T> {
    connection: T,

    connection_reuses: Option<usize>,
}

impl<T> Builder<T> {
    pub fn with_connection_reuses(mut self, value: Option<usize>) -> Self {
        self.connection_reuses = value;

        self
    }
}

mod s2n_quic {
    use s2n_quic::stream::BidirectionalStream;
    use s2n_quic::Connection as NoiseConnection;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    use crate::{debug, error};

    use super::{Builder, Provider, Stream};

    impl<T> Builder<T>
    where
        T: Provider<NoiseConnection> + Send + 'static,
    {
        pub fn new(connection: T) -> Self {
            Self {
                connection,
                connection_reuses: None,
            }
        }

        pub fn build(mut self) -> impl Provider<BidirectionalStream> {
            let (stream_sender, stream_receiver) = mpsc::channel(1usize);

            tokio::spawn(async move {
                'connection: while let Some(mut connection) = self.connection.fetch().await {
                    let mut connection_reuses = 0;

                    'stream: loop {
                        if let Some(value) = self.connection_reuses {
                            if connection_reuses >= value {
                                break 'stream;
                            }
                        }

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

                        if let Err(_error) = stream_sender.send(stream).await {
                            break 'connection;
                        }

                        connection_reuses += 1;
                    }
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
