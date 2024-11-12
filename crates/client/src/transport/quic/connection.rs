use std::net::SocketAddr;

use tokio::sync::mpsc::{self, Receiver};

use crate::{debug, error};

pub mod impl_s2n_quic {
    use s2n_quic::client::{Client as NoiseClient, Connect as NoiseClientConnect};
    use s2n_quic::Connection as NoiseConnection;

    use super::*;

    pub async fn connection(
        client: NoiseClient,
        server_name: String,
        server_address: SocketAddr,
    ) -> Receiver<NoiseConnection> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            let connect =
                NoiseClientConnect::new(server_address).with_server_name(server_name.as_str());

            loop {
                let mut connection = match client.connect(connect.clone()).await {
                    Ok(value) => value,
                    Err(_error) => {
                        error!(
                            "{:?} failed to establish connection with {}. {}",
                            client.local_addr(),
                            server_address,
                            _error
                        );

                        continue;
                    }
                };

                debug!(
                    "{:?} establish connection {} with {:?}",
                    connection.local_addr(),
                    connection.id(),
                    connection.remote_addr()
                );

                if let Err(_error) = connection.keep_alive(true) {
                    error!("failed to keep alive the connection. {}", _error);

                    continue;
                }

                if sender.send(connection).await.is_err() {
                    break;
                }
            }
        });

        receiver
    }
}
