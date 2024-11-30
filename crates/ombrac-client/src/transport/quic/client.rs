use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;

use crate::{error, info};

use super::Config;

pub(crate) mod impl_s2n_quic {
    use std::io;

    use s2n_quic::client::{Client, Connect};
    use s2n_quic::Connection;

    pub use s2n_quic::stream::BidirectionalStream as Stream;

    use super::*;

    pub struct NoiseClient {
        config: Config,
        client: Client,
        connection: Connection,
    }

    impl NoiseClient {
        pub async fn new(config: Config) -> Result<Self, Box<dyn Error>> {
            let server_name = config.server_name()?;
            let server_addr = config.server_socket_address().await?;
            let client = s2n_client_with_config(&config).await?;
            let connect = Connect::new(server_addr).with_server_name(server_name);
            let connection = client.connect(connect).await?;

            Ok(Self {
                config,
                client,
                connection,
            })
        }

        async fn update_connection(&mut self) -> io::Result<()> {
            let server_name = self.config.server_name()?;
            let server_addr = self.config.server_socket_address().await?;
            let connect = Connect::new(server_addr).with_server_name(server_name);

            let mut connection = match self.client.connect(connect).await {
                Ok(value) => value,
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "{} failed to establish connection with {}, {}. {}",
                        self.client.local_addr()?,
                        server_name,
                        server_addr,
                        error
                    )));
                }
            };

            if let Err(error) = connection.keep_alive(true) {
                return Err(io::Error::other(format!(
                    "failed to keep alive the connection {}. {}",
                    connection.id(),
                    error
                )));
            };

            info!(
                "{:?} establish connection {} with {:?}",
                connection.local_addr(),
                connection.id(),
                connection.remote_addr()
            );

            self.connection = connection;

            Ok(())
        }

        pub async fn stream(&mut self) -> Option<Stream> {
            loop {
                let stream = match self.connection.open_bidirectional_stream().await {
                    Ok(stream) => stream,
                    Err(_error) => {
                        error!(
                            "connection {} failed to open bidirectional stream. {}",
                            self.connection.id(),
                            _error
                        );

                        if let Err(_error) = self.update_connection().await {
                            error!("{_error}");
                        };

                        continue;
                    }
                };

                return Some(stream);
            }
        }
    }

    async fn s2n_client_with_config(config: &Config) -> Result<Client, Box<dyn Error>> {
        use s2n_quic::provider::congestion_controller;
        use s2n_quic::provider::limits;

        let limits = {
            let mut limits = limits::Limits::new();

            if let Some(value) = config.bidirectional_local_data_window {
                limits = limits.with_bidirectional_local_data_window(value)?;
            }

            if let Some(value) = config.bidirectional_remote_data_window {
                limits = limits.with_bidirectional_remote_data_window(value)?;
            }

            if let Some(value) = config.max_open_bidirectional_streams {
                limits = limits.with_max_open_local_bidirectional_streams(value)?;
            }

            if let Some(value) = config.max_open_bidirectional_streams {
                limits = limits.with_max_open_remote_bidirectional_streams(value)?;
            }

            if let Some(value) = config.max_handshake_duration {
                limits = limits.with_max_handshake_duration(value)?;
            }

            if let Some(value) = config.max_keep_alive_period {
                limits = limits.with_max_keep_alive_period(value)?;
            }

            if let Some(value) = config.max_idle_timeout {
                limits = limits.with_max_idle_timeout(value)?;
            }

            limits
        };

        let controller = {
            let mut controller = congestion_controller::bbr::Builder::default();

            if let Some(value) = config.initial_congestion_window {
                controller = controller.with_initial_congestion_window(value);
            }

            controller.build()
        };

        let bind_address = match &config.bind {
            Some(value) => value,
            None => match config.server_socket_address().await? {
                SocketAddr::V4(_) => "0.0.0.0:0",
                SocketAddr::V6(_) => "[::]0",
            },
        };

        let client = s2n_quic::Client::builder()
            .with_io(bind_address)?
            .with_limits(limits)?
            .with_congestion_controller(controller)?;

        let client = match &config.tls_cert {
            Some(path) => client.with_tls(Path::new(path))?.start()?,
            None => client.start()?,
        };

        Ok(client)
    }
}
