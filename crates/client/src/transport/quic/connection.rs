use std::net::SocketAddr;

use ombrac_protocol::Provider;

pub struct Connection<T> {
    inner: T,
}

pub struct Builder<T> {
    client: T,
    server_name: String,
    server_addr: SocketAddr,
}

mod s2n_quic {
    use s2n_quic::{
        client::{Client as NoiseClient, Connect as NoiseConnect},
        Connection as NoiseConnection,
    };
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    use crate::{debug, error};

    use super::{Builder, Connection, Provider, SocketAddr};

    impl<T> Builder<T> {
        pub fn new<U>(client: T, server_name: String, server_addr: U) -> Self
        where
            U: Into<SocketAddr>,
        {
            Self {
                client,
                server_name,
                server_addr: server_addr.into(),
            }
        }

        pub fn build(self) -> impl Provider<NoiseConnection>
        where
            T: IntoIterator<Item = NoiseClient> + Send + 'static,
            <T as IntoIterator>::IntoIter: Clone + Send,
        {
            let (connection_sender, connection_receiver) = mpsc::channel(1usize);
            let server_name = self.server_name;
            let server_addr = self.server_addr;
            let clients = self.client.into_iter().cycle();

            tokio::spawn(async move {
                'connection: for client in clients {
                    if let Some(connection) = connect(&client, &server_name, server_addr).await {
                        if connection_sender.send(connection).await.is_err() {
                            break 'connection;
                        }
                    }
                }
            });

            Connection {
                inner: connection_receiver,
            }
        }
    }

    async fn connect(
        client: &NoiseClient,
        server_name: &str,
        server_addr: SocketAddr,
    ) -> Option<NoiseConnection> {
        let connect = NoiseConnect::new(server_addr).with_server_name(server_name);
        let mut connection = match client.connect(connect).await {
            Ok(value) => value,
            Err(_error) => {
                error!(
                    "{:?} failed to establish connection with {}. {}",
                    client.local_addr(),
                    server_name,
                    _error
                );
                return None;
            }
        };

        if let Err(_error) = connection.keep_alive(true) {
            error!("failed to keep alive the connection. {}", _error);
            return None;
        }

        debug!(
            "{:?} establish connection {} with {:?}",
            connection.local_addr(),
            connection.id(),
            connection.remote_addr()
        );

        Some(connection)
    }

    impl Provider<NoiseConnection> for Connection<Receiver<NoiseConnection>> {
        async fn fetch(&mut self) -> Option<NoiseConnection> {
            self.inner.recv().await
        }
    }
}
