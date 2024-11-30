use std::error::Error;

use crate::info;
use crate::Server;

use super::Config;

mod impl_s2n_quic {
    use std::path::Path;

    use s2n_quic::provider::congestion_controller;
    use s2n_quic::provider::limits;
    use s2n_quic::stream::BidirectionalStream as Stream;
    use s2n_quic::Server as NoiseServer;

    use super::*;

    impl Server<NoiseServer> {
        pub fn new(config: Config) -> Result<Self, Box<dyn Error>> {
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

            let server = NoiseServer::builder()
                .with_io(config.listen)?
                .with_limits(limits)?
                .with_congestion_controller(controller)?
                .with_tls((Path::new(&config.tls_cert), Path::new(&config.tls_key)))?
                .start()?;

            Ok(Self { inner: server })
        }
    }

    impl ombrac::Server<Stream> for Server<NoiseServer> {
        async fn listen(mut self) -> () {
            while let Some(mut connection) = self.inner.accept().await {
                tokio::spawn(async move {
                    info!(
                        "{:?} accept connection {} from {:?}",
                        connection.server_name(),
                        connection.id(),
                        connection.remote_addr()
                    );

                    while let Ok(Some(stream)) = connection.accept_bidirectional_stream().await {
                        tokio::spawn(async move { Self::handler(stream).await });
                    }
                });
            }
        }
    }
}
