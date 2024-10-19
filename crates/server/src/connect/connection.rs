use ombrac_protocol::Provider;

pub struct Connection<T> {
    inner: T,
}

pub struct Builder<T> {
    server: T,
}

mod s2n_quic {
    use s2n_quic::{Connection as NoiseConnection, Server as NoiseServer};
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    use super::{Builder, Connection, Provider};

    impl Builder<NoiseServer> {
        pub fn new(server: NoiseServer) -> Self {
            Self { server }
        }

        pub fn build(self) -> impl Provider<NoiseConnection> {
            let (connection_sender, connection_receiver) = mpsc::channel(1);
            let mut server = self.server;

            tokio::spawn(async move {
                while let Some(connection) = server.accept().await {
                    if connection_sender.send(connection).await.is_err() {
                        break;
                    }
                }
            });

            Connection {
                inner: connection_receiver,
            }
        }
    }

    impl Provider<NoiseConnection> for Connection<Receiver<NoiseConnection>> {
        async fn fetch(&mut self) -> Option<NoiseConnection> {
            self.inner.recv().await
        }
    }
}
